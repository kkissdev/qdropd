//! The set of currently-trusted peers, shared between the TLS verifier, the
//! dialer, and discovery filtering. Rebuilt from `peers.toml` whenever that
//! file changes, without tearing down the TLS config.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::Peers;

/// One trusted peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterPeer {
    pub device_id: String,
    pub name: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Default)]
pub struct RosterInner {
    by_key: HashMap<[u8; 32], RosterPeer>,
    by_id: HashMap<String, [u8; 32]>,
}

impl RosterInner {
    pub fn is_trusted_key(&self, key: &[u8; 32]) -> bool {
        self.by_key.contains_key(key)
    }

    pub fn is_trusted_id(&self, device_id: &str) -> bool {
        self.by_id.contains_key(device_id)
    }

    pub fn key_for_id(&self, device_id: &str) -> Option<[u8; 32]> {
        self.by_id.get(device_id).copied()
    }

    pub fn peer_for_key(&self, key: &[u8; 32]) -> Option<&RosterPeer> {
        self.by_key.get(key)
    }

    pub fn ids(&self) -> Vec<String> {
        self.by_id.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    fn replace(&mut self, peers: Vec<RosterPeer>) {
        self.by_key.clear();
        self.by_id.clear();
        for p in peers {
            self.by_id.insert(p.device_id.clone(), p.public_key);
            self.by_key.insert(p.public_key, p);
        }
    }
}

/// Shared, mutable roster handle.
#[derive(Clone, Debug)]
pub struct Roster(pub Arc<RwLock<RosterInner>>);

impl Default for Roster {
    fn default() -> Self {
        Self::new()
    }
}

impl Roster {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(RosterInner::default())))
    }

    /// Build from a `peers.toml` document (skips entries with unusable keys).
    pub fn from_peers(peers: &Peers) -> Self {
        let r = Self::new();
        r.load(peers);
        r
    }

    /// Replace the contents from a `peers.toml` document.
    pub fn load(&self, peers: &Peers) {
        let mut list = Vec::new();
        for p in &peers.peers {
            match p.key_bytes() {
                Ok(key) => list.push(RosterPeer {
                    device_id: p.device_id.clone(),
                    name: p.name.clone(),
                    public_key: key,
                }),
                Err(e) => tracing::warn!("skipping peer {}: {e}", p.name),
            }
        }
        self.0.write().unwrap().replace(list);
    }

    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, RosterInner> {
        self.0.read().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Peer;

    #[test]
    fn builds_both_indexes_and_reloads() {
        let mut peers = Peers::default();
        peers.upsert(Peer::new("a", "id-a", &[1u8; 32]));
        peers.upsert(Peer::new("b", "id-b", &[2u8; 32]));

        let roster = Roster::from_peers(&peers);
        {
            let r = roster.read();
            assert_eq!(r.len(), 2);
            assert!(r.is_trusted_id("id-a"));
            assert!(r.is_trusted_key(&[2u8; 32]));
            assert_eq!(r.key_for_id("id-b"), Some([2u8; 32]));
            assert_eq!(r.peer_for_key(&[1u8; 32]).unwrap().name, "a");
        }

        peers.remove_by_name("a");
        roster.load(&peers);
        let r = roster.read();
        assert_eq!(r.len(), 1);
        assert!(!r.is_trusted_id("id-a"));
        assert!(!r.is_trusted_key(&[1u8; 32]));
    }
}
