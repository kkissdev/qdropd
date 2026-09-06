//! Publishes connection state to `~/.config/qdrop/state.json` so `qdrop peers`
//! can show online / last-seen without a control socket (that arrives in M7).

use std::sync::Arc;

use qdrop_core::state::DaemonState;
use tokio::sync::mpsc;

use crate::auth::AuthManager;
use crate::transport::TransportEvent;

/// Consume transport events, updating `state.json` on every change.
pub async fn publish_events(mut rx: mpsc::Receiver<TransportEvent>, auth: Arc<AuthManager>) {
    let path = match qdrop_core::paths::state_file() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("cannot locate state file: {e:#}");
            return;
        }
    };
    let mut state = DaemonState::load_from(&path).unwrap_or_default();
    // A fresh daemon has no live connections yet.
    state.online.clear();
    let _ = state.save_to(&path);

    while let Some(ev) = rx.recv().await {
        match ev {
            TransportEvent::PeerConnected {
                device_id,
                device_name,
                caps,
                addr,
            } => {
                tracing::info!(peer = %device_id, name = %device_name, caps = ?caps, "peer online");
                state.mark_online(&device_id);
                if let Some(addr) = addr {
                    state.remember_addr(&device_id, &addr);
                }
                // Refresh the recorded label / warn on a MAC change (M10).
                let auth = auth.clone();
                let id = device_id.clone();
                tokio::spawn(async move { auth.refresh(id).await });
            }
            TransportEvent::PeerDisconnected { device_id, reason } => {
                tracing::info!(peer = %device_id, %reason, "peer offline");
                state.mark_offline(&device_id);
            }
        }
        if let Err(e) = state.save_to(&path) {
            tracing::warn!("writing state.json failed: {e:#}");
        }
    }
}
