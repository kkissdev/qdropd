//! `peers.toml` — the registry of paired devices, written by `qdrop pair`.

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
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
    /// Pre-authorized as an unattended sender (`qdrop auth`, M10).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub authorized: bool,
    /// MAC addresses recorded during authorization (a label + soft check).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macs: Vec<String>,
    /// Hostname recorded during authorization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// Static address the dialer should try in addition to mDNS discovery,
    /// for peers on a different link (e.g. reachable only over a VPN/tailnet).
    /// Accepts `host`, `host:port`, `ip`, or `ip:port`; a missing port means
    /// [`crate::DEFAULT_PORT`]. Names are resolved when the roster loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

impl Peer {
    /// Build a peer entry from a raw public key.
    pub fn new(
        name: impl Into<String>,
        device_id: impl Into<String>,
        public_key: &[u8; 32],
    ) -> Self {
        Self {
            name: name.into(),
            device_id: device_id.into(),
            public_key: STANDARD_NO_PAD.encode(public_key),
            last_seen: None,
            authorized: false,
            macs: Vec::new(),
            hostname: None,
            address: None,
        }
    }

    /// Resolve [`Peer::address`] to socket addresses, or `[]` if unset or
    /// unresolvable. A bare host/ip gets [`crate::DEFAULT_PORT`].
    pub fn static_addrs(&self) -> Vec<SocketAddr> {
        let Some(raw) = self
            .address
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Vec::new();
        };
        if let Ok(sa) = raw.parse::<SocketAddr>() {
            return vec![sa];
        }
        if let Ok(ip) = raw.parse::<std::net::IpAddr>() {
            return vec![SocketAddr::new(ip, crate::DEFAULT_PORT)];
        }
        let with_port = if raw
            .rsplit(':')
            .next()
            .is_some_and(|p| p.parse::<u16>().is_ok())
        {
            raw.to_string()
        } else {
            format!("{raw}:{}", crate::DEFAULT_PORT)
        };
        match with_port.to_socket_addrs() {
            Ok(iter) => iter.collect(),
            Err(e) => {
                tracing::warn!("peer {}: cannot resolve address {raw:?}: {e}", self.name);
                Vec::new()
            }
        }
    }

    /// Decode the pinned public key.
    pub fn key_bytes(&self) -> Result<[u8; 32]> {
        let raw = STANDARD_NO_PAD
            .decode(self.public_key.trim())
            .with_context(|| format!("decoding public key for peer {}", self.name))?;
        let arr: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("public key for peer {} is not 32 bytes", self.name))?;
        Ok(arr)
    }
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

    /// Save to the default location, atomically.
    pub fn save(&self) -> Result<()> {
        self.save_to(crate::paths::peers_file()?)
    }

    /// Save to a specific path, atomically (temp file + rename).
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let text = toml::to_string_pretty(self).context("serializing peers")?;
        crate::paths::write_atomic(path.as_ref(), text.as_bytes())
    }

    /// Look up a peer by name.
    pub fn find(&self, name: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.name == name)
    }

    /// Look up a peer by device id.
    pub fn find_by_id(&self, device_id: &str) -> Option<&Peer> {
        self.peers.iter().find(|p| p.device_id == device_id)
    }

    /// Insert `peer`, replacing any existing entry with the same `device_id`.
    /// If the chosen name collides with a *different* device, the new entry's
    /// name gets a short suffix so `--to <name>` stays unambiguous.
    pub fn upsert(&mut self, mut peer: Peer) {
        self.peers.retain(|p| p.device_id != peer.device_id);
        if self
            .peers
            .iter()
            .any(|p| p.name == peer.name && p.device_id != peer.device_id)
        {
            let suffix: String = peer.device_id.chars().take(6).collect();
            peer.name = format!("{}-{}", peer.name, suffix);
        }
        self.peers.push(peer);
    }

    /// Remove the peer with this name. Returns whether one was removed.
    pub fn remove_by_name(&mut self, name: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.name != name);
        self.peers.len() != before
    }

    /// Look up by name or device id (accepts either).
    pub fn find_any_mut(&mut self, name_or_id: &str) -> Option<&mut Peer> {
        self.peers
            .iter_mut()
            .find(|p| p.name == name_or_id || p.device_id == name_or_id)
    }

    /// Whether a peer (by device id) is pre-authorized as an unattended sender.
    pub fn is_authorized(&self, device_id: &str) -> bool {
        self.find_by_id(device_id).is_some_and(|p| p.authorized)
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
    fn static_addr_parsing() {
        let mut p = Peer::new("box", "id", &[0u8; 32]);
        assert!(p.static_addrs().is_empty());

        p.address = Some("100.67.13.80".into());
        assert_eq!(
            p.static_addrs(),
            vec!["100.67.13.80:47654".parse().unwrap()]
        );

        p.address = Some("100.67.13.80:51000".into());
        assert_eq!(
            p.static_addrs(),
            vec!["100.67.13.80:51000".parse().unwrap()]
        );

        p.address = Some("  ".into());
        assert!(p.static_addrs().is_empty());

        p.address = Some("[::1]:9".into());
        assert_eq!(p.static_addrs(), vec!["[::1]:9".parse().unwrap()]);

        p.address = Some("::1".into());
        assert_eq!(p.static_addrs(), vec!["[::1]:47654".parse().unwrap()]);
    }

    #[test]
    fn address_roundtrips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");
        let mut peers = Peers::default();
        let mut peer = Peer::new("ws", "id-ws", &[5u8; 32]);
        peer.address = Some("100.67.13.80".into());
        peers.upsert(peer);
        peers.save_to(&path).unwrap();

        let back = Peers::load_from(&path).unwrap();
        assert_eq!(
            back.find("ws").unwrap().address.as_deref(),
            Some("100.67.13.80")
        );
    }

    #[test]
    fn key_roundtrips_through_base64() {
        let key = [7u8; 32];
        let p = Peer::new("loki", "id1", &key);
        assert_eq!(p.key_bytes().unwrap(), key);
    }

    #[test]
    fn roundtrips_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("peers.toml");

        let mut peers = Peers::default();
        peers.upsert(Peer::new("loki", "01HXYZ", &[1u8; 32]));
        peers.save_to(&path).unwrap();

        let back = Peers::load_from(&path).unwrap();
        assert_eq!(peers, back);
        assert_eq!(back.find("loki").unwrap().device_id, "01HXYZ");
    }

    #[test]
    fn upsert_replaces_same_device_and_disambiguates_names() {
        let mut peers = Peers::default();
        peers.upsert(Peer::new("box", "aaa", &[1u8; 32]));
        peers.upsert(Peer::new("box", "aaa", &[2u8; 32])); // same device, new key
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(
            peers.find_by_id("aaa").unwrap().key_bytes().unwrap(),
            [2u8; 32]
        );

        peers.upsert(Peer::new("box", "bbbbbbbb", &[3u8; 32])); // different device, same name
        assert_eq!(peers.peers.len(), 2);
        assert!(peers.find("box").is_some());
        assert!(peers.find("box-bbbbbb").is_some());
    }

    #[test]
    fn remove_by_name() {
        let mut peers = Peers::default();
        peers.upsert(Peer::new("a", "ida", &[1u8; 32]));
        assert!(peers.remove_by_name("a"));
        assert!(!peers.remove_by_name("a"));
        assert!(peers.is_empty());
    }

    #[test]
    fn authorization_flag_roundtrips_and_lookup_by_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");
        let mut peers = Peers::default();
        peers.upsert(Peer::new("laptop", "id-laptop", &[9u8; 32]));

        assert!(!peers.is_authorized("id-laptop"));
        let p = peers.find_any_mut("laptop").unwrap();
        p.authorized = true;
        p.hostname = Some("laptop.local".into());
        p.macs = vec!["aa:bb:cc:dd:ee:ff".into()];
        peers.save_to(&path).unwrap();

        let mut back = Peers::load_from(&path).unwrap();
        assert!(back.is_authorized("id-laptop"));
        assert_eq!(
            back.find_by_id("id-laptop").unwrap().hostname.as_deref(),
            Some("laptop.local")
        );
        assert!(back.find_any_mut("id-laptop").is_some());
    }
}
