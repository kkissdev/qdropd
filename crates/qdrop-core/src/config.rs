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
    /// Prompt before accepting an incoming file.
    pub require_confirm: bool,
    /// Default log filter when `--verbose` is not passed (`tracing` syntax).
    pub log_filter: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            port: DEFAULT_PORT,
            sync_clipboard: true,
            sync_images: true,
            max_clipboard_bytes: 1024 * 1024,
            require_confirm: false,
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
}
