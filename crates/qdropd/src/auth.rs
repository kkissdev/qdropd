//! `qdrop auth` — pre-authorize a paired peer as an unattended sender (M10).
//!
//! The pinned Ed25519 key is the security boundary. This exchange only records
//! a human-readable label (hostname + MAC addresses) and flips an `authorized`
//! flag in `peers.toml` so [`crate::filexfer`] / [`crate::weblink`] stop
//! diverting that peer's transfers for confirmation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use qdrop_core::proto::{DeviceInfo, Message};
use qdrop_core::sysinfo::{local_device_info, macs_still_match};
use qdrop_core::Peers;
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;

use crate::bus::PeerBus;
use crate::notify::notify;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AuthManager {
    bus: PeerBus,
    /// peer id -> waiter for that peer's `AuthReply`.
    pending: Mutex<HashMap<String, oneshot::Sender<DeviceInfo>>>,
}

impl AuthManager {
    pub fn new(bus: PeerBus) -> Arc<Self> {
        Arc::new(Self {
            bus,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Route an inbound `AuthRequest` / `AuthReply` here.
    pub async fn handle_inbound(self: &Arc<Self>, peer_id: String, msg: Message) {
        match msg {
            Message::AuthRequest { mutual, info } => {
                let _ = self.bus.send_to(
                    &peer_id,
                    Message::AuthReply {
                        ok: true,
                        info: local_device_info(),
                    },
                );
                if mutual {
                    if let Err(e) = record(&peer_id, &info, true) {
                        tracing::warn!(peer = %peer_id, "mutual-auth record failed: {e:#}");
                    } else {
                        tracing::info!(peer = %peer_id, host = %info.hostname, "authorized peer (mutual)");
                        notify(
                            "Device authorized",
                            &format!("{} can now send here unattended", info.hostname),
                        );
                    }
                }
            }
            Message::AuthReply { ok: _, info } => {
                if let Some(tx) = self.pending.lock().await.remove(&peer_id) {
                    let _ = tx.send(info);
                }
            }
            _ => {}
        }
    }

    /// Ask `peer_id` to authorize us (and, if `mutual`, authorize it back).
    /// Records the result in `peers.toml`.
    pub async fn authorize(self: &Arc<Self>, peer_id: &str, mutual: bool) -> Result<String> {
        anyhow::ensure!(
            self.bus.sender(peer_id).is_some(),
            "peer is not connected — start both daemons and try again"
        );
        let info = self.exchange(peer_id, mutual).await?;
        record(peer_id, &info, true)?;
        Ok(info.hostname)
    }

    /// Silent re-check on reconnect: refresh the recorded label and warn if the
    /// peer's MACs no longer overlap what we saw before. Never revokes.
    pub async fn refresh(self: &Arc<Self>, peer_id: String) {
        let recorded = match Peers::load().ok().and_then(|p| {
            p.find_by_id(&peer_id)
                .filter(|p| p.authorized)
                .map(|p| p.macs.clone())
        }) {
            Some(m) => m,
            None => return, // not authorized
        };
        let Ok(info) = self.exchange(&peer_id, false).await else {
            return; // old peer / no reply — nothing to do
        };
        if !macs_still_match(&recorded, &info.macs) {
            tracing::warn!(
                peer = %peer_id, host = %info.hostname,
                "authorized peer connected from a new network (MAC changed) — still trusted (key pinned)"
            );
            notify(
                "Authorized device on a new network",
                &format!("{} — still trusted (its key is pinned)", info.hostname),
            );
        }
        let _ = record(&peer_id, &info, true);
    }

    async fn exchange(self: &Arc<Self>, peer_id: &str, mutual: bool) -> Result<DeviceInfo> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(peer_id.to_string(), tx);

        let queued = self.bus.send_to(
            peer_id,
            Message::AuthRequest {
                mutual,
                info: local_device_info(),
            },
        );
        if !queued {
            self.pending.lock().await.remove(peer_id);
            anyhow::bail!("peer is not connected");
        }

        let out = timeout(REPLY_TIMEOUT, rx).await;
        self.pending.lock().await.remove(peer_id);
        match out {
            Ok(Ok(info)) => Ok(info),
            Ok(Err(_)) => anyhow::bail!("authorization channel closed"),
            Err(_) => anyhow::bail!("peer did not reply (it may be running an older qdrop)"),
        }
    }
}

/// Write `authorized` + the peer's label into `peers.toml`.
fn record(device_id: &str, info: &DeviceInfo, authorized: bool) -> Result<()> {
    let mut peers = Peers::load()?;
    let peer = peers
        .find_any_mut(device_id)
        .context("peer not found in peers.toml (is it paired?)")?;
    peer.authorized = authorized;
    peer.hostname = Some(info.hostname.clone());
    peer.macs = info.macs.clone();
    peers.save()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn request_gets_a_reply_with_our_device_info() {
        let (tx, mut rx) = mpsc::channel(4);
        let bus = PeerBus::new();
        bus.insert("peer".into(), tx);
        let auth = AuthManager::new(bus);

        // Peer sends us an AuthRequest -> we must answer with an AuthReply.
        auth.handle_inbound(
            "peer".into(),
            Message::AuthRequest {
                mutual: false,
                info: DeviceInfo {
                    hostname: "them".into(),
                    macs: vec![],
                    os: "linux".into(),
                },
            },
        )
        .await;

        match rx.recv().await.unwrap() {
            Message::AuthReply { ok, info } => {
                assert!(ok);
                assert!(!info.hostname.is_empty());
                assert_eq!(info.os, std::env::consts::OS);
            }
            other => panic!("expected AuthReply, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn authorize_errors_when_peer_not_connected() {
        let auth = AuthManager::new(PeerBus::new());
        let err = auth.authorize("ghost", false).await.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }
}
