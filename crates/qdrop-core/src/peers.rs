//! `peers.toml` — the registry of paired devices.
//!
//! M0 only needs to read and write this file; pairing (which populates it)
//! lands in M2.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single paired peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    /// Friendly name (used for `--to <name>` targeting).
    pub name: String,
    /// Stable device identifier.
    pub device_id: String,
    /// Base64 (standard, no padding) Ed25519 public key, pinned for TLS auth.
    pub public_key: String,
    /// Last successful contact, RFC 3339. `None` until first connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// The whole `peers.toml` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Peers {
    #[serde(rename = "peer")]
    pub peers: Vec<Peer>,
}

impl Peers {
    /// Load from the default location, returning an empty registry if absent.
    pub fn load() -> Result<Self> {
        Self::load_from(crate::paths::peers_file()?)
    }

    /// Load from a specific path, returning an empty registry if absent.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("parsing peers file {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading peers file {}", path.display())),
        }
    }

    /// Write to a specific path, creating parent directories as needed.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing peers")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Look up a peer by name.
    pub fn find(&self, name: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_registry() {
        let peers = Peers::load_from("/no/such/peers.toml").unwrap();
        assert!(peers.is_empty());
    }

    #[test]
    fn roundtrips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("peers.toml");

        let peers = Peers {
            peers: vec![Peer {
                name: "loki".into(),
                device_id: "01HXYZ".into(),
                public_key: "abc123".into(),
                last_seen: None,
            }],
        };
        peers.save_to(&path).unwrap();

        let back = Peers::load_from(&path).unwrap();
        assert_eq!(peers, back);
        assert_eq!(back.find("loki").unwrap().device_id, "01HXYZ");
        assert!(back.find("thor").is_none());
    }
}
