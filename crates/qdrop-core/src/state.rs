//! `state.json` — the daemon's live view, published for `qdrop peers`.
//!
//! This is a stop-gap until the daemon exposes a proper control socket (M7).
//! The daemon owns the file; the CLI only reads it.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Seconds since the Unix epoch, now.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonState {
    /// device id -> unix time the current connection was established.
    #[serde(default)]
    pub online: BTreeMap<String, u64>,
    /// device id -> unix time last seen (updated on disconnect).
    #[serde(default)]
    pub last_seen: BTreeMap<String, u64>,
    /// device id -> last address we successfully connected to. Used to dial a
    /// peer directly before mDNS has re-resolved it (network transitions,
    /// flaky multicast).
    #[serde(default)]
    pub last_addr: BTreeMap<String, String>,
}

impl DaemonState {
    pub fn load() -> Result<Self> {
        Self::load_from(crate::paths::state_file()?)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(crate::paths::state_file()?)
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).context("serializing daemon state")?;
        crate::paths::write_atomic(path.as_ref(), &bytes)
    }

    pub fn mark_online(&mut self, device_id: &str) {
        self.online.insert(device_id.to_string(), now_unix());
    }

    pub fn remember_addr(&mut self, device_id: &str, addr: &str) {
        self.last_addr
            .insert(device_id.to_string(), addr.to_string());
    }

    pub fn mark_offline(&mut self, device_id: &str) {
        self.online.remove(device_id);
        self.last_seen.insert(device_id.to_string(), now_unix());
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.online.contains_key(device_id)
    }
}

/// Render a unix timestamp as a coarse "time ago" string.
pub fn ago(then_unix: u64) -> String {
    let now = now_unix();
    if then_unix == 0 || then_unix > now {
        return "unknown".to_string();
    }
    let secs = now - then_unix;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut st = DaemonState::default();
        st.mark_online("dev1");
        st.save_to(&path).unwrap();
        assert!(DaemonState::load_from(&path).unwrap().is_online("dev1"));

        st.mark_offline("dev1");
        st.save_to(&path).unwrap();
        let back = DaemonState::load_from(&path).unwrap();
        assert!(!back.is_online("dev1"));
        assert!(back.last_seen.contains_key("dev1"));
    }

    #[test]
    fn ago_buckets() {
        let now = now_unix();
        assert!(ago(now.saturating_sub(5)).ends_with("s ago"));
        assert!(ago(now.saturating_sub(120)).ends_with("m ago"));
        assert_eq!(ago(0), "unknown");
    }
}
