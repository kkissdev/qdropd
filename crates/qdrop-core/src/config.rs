//! `config.toml` loading with defaults.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::DEFAULT_PORT;

/// Top-level daemon/CLI configuration.
///
/// Every field has a default, so an absent or partial `config.toml` still
/// yields a usable `Config`. Unknown keys are rejected to catch typos early.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Human-readable name advertised to peers. Defaults to the OS hostname.
    pub device_name: String,
    /// TCP port the daemon listens on.
    pub port: u16,
    /// Clipboard sync master switch.
    pub sync_clipboard: bool,
    /// Whether images (not just text) sync via the clipboard.
    pub sync_images: bool,
    /// Largest clipboard payload to sync inline, in bytes.
    pub max_clipboard_bytes: u64,
    /// When to divert an incoming file/URL for confirmation.
    /// Accepts `false` / `true` / `"strict"` in `config.toml`.
    pub require_confirm: ConfirmPolicy,
    /// Default log filter when `--verbose` is not passed (`tracing` syntax).
    pub log_filter: String,
}

/// When incoming files (M4) and URLs (M5) are held for confirmation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfirmPolicy {
    /// Accept everything from any paired peer. (`false`)
    #[default]
    Never,
    /// Accept from peers authorized via `qdrop auth` (M10); divert the rest to
    /// `~/Downloads/qdrop/pending/`. (`true`)
    Unlisted,
    /// Always divert, authorized or not. (`"strict"`)
    Always,
}

impl serde::Serialize for ConfirmPolicy {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ConfirmPolicy::Never => s.serialize_bool(false),
            ConfirmPolicy::Unlisted => s.serialize_bool(true),
            ConfirmPolicy::Always => s.serialize_str("strict"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ConfirmPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Str(String),
        }
        match Raw::deserialize(d)? {
            Raw::Bool(false) => Ok(ConfirmPolicy::Never),
            Raw::Bool(true) => Ok(ConfirmPolicy::Unlisted),
            Raw::Str(s) => match s.to_ascii_lowercase().as_str() {
                "never" | "false" => Ok(ConfirmPolicy::Never),
                "unlisted" | "true" => Ok(ConfirmPolicy::Unlisted),
                "strict" | "always" => Ok(ConfirmPolicy::Always),
                other => Err(serde::de::Error::custom(format!(
                    "require_confirm: expected false / true / \"strict\", got {other:?}"
                ))),
            },
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            port: DEFAULT_PORT,
            sync_clipboard: true,
            sync_images: true,
            max_clipboard_bytes: 1024 * 1024,
            require_confirm: ConfirmPolicy::Never,
            log_filter: "info".to_string(),
        }
    }
}

impl Config {
    /// Load from the default location ([`crate::paths::config_file`]),
    /// returning defaults if the file does not exist.
    pub fn load() -> Result<Self> {
        Self::load_from(crate::paths::config_file()?)
    }

    /// Load from a specific path, returning defaults if it does not exist.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("parsing config file {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading config file {}", path.display())),
        }
    }

    /// Serialize to a TOML string (used to write a starter config).
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("serializing config")
    }
}

fn default_device_name() -> String {
    std::env::var("QDROP_DEVICE_NAME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(hostname)
        .unwrap_or_else(|| "qdrop-device".to_string())
}

fn hostname() -> Option<String> {
    // Avoid a dependency for one syscall; `hostname` is on PATH everywhere we target.
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let cfg = Config::load_from("/no/such/qdrop/config.toml").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_file_merges_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "port = 12345\nsync_images = false\n").unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.port, 12345);
        assert!(!cfg.sync_images);
        // Untouched fields keep their defaults.
        assert_eq!(
            cfg.max_clipboard_bytes,
            Config::default().max_clipboard_bytes
        );
    }

    #[test]
    fn unknown_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "prot = 1\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn roundtrips_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml().unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn confirm_policy_accepts_bool_and_strict() {
        let p = |s: &str| toml::from_str::<Config>(s).unwrap().require_confirm;
        assert_eq!(p("require_confirm = false"), ConfirmPolicy::Never);
        assert_eq!(p("require_confirm = true"), ConfirmPolicy::Unlisted);
        assert_eq!(p("require_confirm = \"strict\""), ConfirmPolicy::Always);
        assert_eq!(p("require_confirm = \"unlisted\""), ConfirmPolicy::Unlisted);
        assert!(toml::from_str::<Config>("require_confirm = \"bogus\"").is_err());

        // Round-trips: Always -> "strict", Unlisted -> true.
        let c = Config {
            require_confirm: ConfirmPolicy::Always,
            ..Config::default()
        };
        assert_eq!(
            toml::from_str::<Config>(&c.to_toml().unwrap())
                .unwrap()
                .require_confirm,
            ConfirmPolicy::Always
        );
    }
}
