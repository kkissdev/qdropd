//! Clipboard text sync: local copy → broadcast; remote update → local apply.
//!
//! Echo suppression is layered: the clipboard thread drops the echo of text it
//! just wrote (by hash); here we ignore a remote update whose `origin_id` is
//! us, and we never re-broadcast text we just received. Together these stop
//! the A→B→A oscillation the milestone warns about.

use std::sync::Arc;
use std::time::Duration;

use qdrop_core::crypto::sha256;
use qdrop_core::proto::{ClipEntry, Message};
use qdrop_core::Config;
use tokio::sync::mpsc;

use crate::bus::PeerBus;
use crate::clipboard::{self, ClipboardHandle};
use crate::control::Controls;

/// Wire up clipboard sync. Returns immediately; work happens in spawned tasks.
pub fn spawn(
    config: &Config,
    own_id: String,
    bus: PeerBus,
    mut inbound: mpsc::Receiver<(String, Message)>,
    controls: Arc<Controls>,
) {
    let (local_tx, mut local_rx) = mpsc::channel::<String>(16);
    let clip: Option<ClipboardHandle> = if config.sync_clipboard {
        clipboard::start(Duration::from_millis(250), local_tx)
    } else {
        tracing::info!("clipboard sync disabled by config");
        None
    };

    let max_bytes = config.max_clipboard_bytes as usize;
    let send_enabled = config.sync_clipboard;
    let recv_enabled = config.sync_clipboard;

    tokio::spawn(async move {
        let mut seq: u64 = 0;
        // Hash of the last text we applied or sent, so we don't bounce it.
        let mut last_synced: Option<[u8; 32]> = None;

        loop {
            tokio::select! {
                // Local copy detected.
                Some(text) = local_rx.recv() => {
                    if !send_enabled || controls.clipboard_paused() {
                        continue;
                    }
                    if text.len() > max_bytes {
                        tracing::debug!(bytes = text.len(), max = max_bytes, "clipboard payload over limit, not syncing");
                        continue;
                    }
                    let h = sha256(text.as_bytes());
                    if Some(h) == last_synced {
                        continue;
                    }
                    last_synced = Some(h);
                    seq += 1;
                    let msg = Message::Clipboard {
                        seq,
                        origin_id: own_id.clone(),
                        entries: vec![ClipEntry::text(&text)],
                    };
                    let n = bus.connected_ids().len();
                    bus.broadcast(&msg, None);
                    tracing::info!(seq, peers = n, bytes = text.len(), "clipboard broadcast");
                }

                // Remote update from a peer.
                Some((peer_id, msg)) = inbound.recv() => {
                    let Message::Clipboard { origin_id, entries, .. } = msg else { continue };
                    if !recv_enabled || controls.clipboard_paused() {
                        continue;
                    }
                    if origin_id == own_id {
                        continue; // our own frame reflected by a relay — ignore
                    }
                    let Some(text) = entries.iter().find_map(|e| e.as_text()) else { continue };
                    if text.len() > max_bytes {
                        continue;
                    }
                    let h = sha256(text.as_bytes());
                    if Some(h) == last_synced {
                        continue; // already have this exact content
                    }
                    last_synced = Some(h);
                    if let Some(clip) = &clip {
                        clip.apply(text.to_string());
                        tracing::info!(from = %peer_id, bytes = text.len(), "clipboard applied from peer");
                    }
                }

                else => break,
            }
        }
    });
}
