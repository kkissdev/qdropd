//! Clipboard sync (text + images, M3/M6).
//!
//! Local copy → broadcast to peers; remote update → apply locally. Echo
//! suppression is layered: the clipboard thread drops the echo of what it just
//! wrote (by hash); here we ignore remote updates whose `origin_id` is us and
//! never re-broadcast content we just received. Concealed / transient
//! pasteboard items (password managers) never leave the clipboard thread.

use std::sync::Arc;
use std::time::Duration;

use qdrop_core::crypto::sha256;
use qdrop_core::proto::{ClipEntry, Message, CLIP_IMAGE_INLINE_MAX};
use qdrop_core::Config;
use tokio::sync::mpsc;

use crate::bus::PeerBus;
use crate::clipboard::{self, ClipboardHandle, LocalClip};
use crate::control::Controls;
use crate::filexfer::FileXfer;

/// Wire up clipboard sync. Returns the clipboard handle (for `qdrop paste` /
/// `qdrop copy`); work happens in spawned tasks.
pub fn spawn(
    config: &Config,
    own_id: String,
    bus: PeerBus,
    filex: Arc<FileXfer>,
    mut clip_frames: mpsc::Receiver<(String, Message)>,
    mut blob_images: mpsc::Receiver<Vec<u8>>,
    controls: Arc<Controls>,
) -> Option<Arc<ClipboardHandle>> {
    controls.set_sync_images(config.sync_images);
    let (local_tx, mut local_rx) = mpsc::channel::<LocalClip>(16);
    let clip: Option<Arc<ClipboardHandle>> = if config.sync_clipboard {
        let c = controls.clone();
        clipboard::start(
            Duration::from_millis(250),
            move || c.sync_images(),
            local_tx,
        )
        .map(Arc::new)
    } else {
        tracing::info!("clipboard sync disabled by config");
        None
    };
    let handle = clip.clone();

    let max_text = config.max_clipboard_bytes as usize;

    // Apply blob-delivered clipboard images.
    if let Some(clip) = clip.clone() {
        tokio::spawn(async move {
            while let Some(png) = blob_images.recv().await {
                clip.apply_image(png);
            }
        });
    }

    tokio::spawn(async move {
        let mut seq: u64 = 0;
        let mut last_synced: Option<[u8; 32]> = None;
        // Rate-limit outbound broadcasts so a script hammering the clipboard
        // can't saturate the link.
        let min_gap = Duration::from_millis(300);
        let mut last_broadcast = tokio::time::Instant::now() - min_gap;

        loop {
            tokio::select! {
                Some(clip_local) = local_rx.recv() => {
                    if controls.clipboard_paused() {
                        continue;
                    }
                    if last_broadcast.elapsed() < min_gap {
                        tracing::debug!("clipboard change rate-limited");
                        continue;
                    }
                    last_broadcast = tokio::time::Instant::now();
                    match clip_local {
                        LocalClip::Text(text) => {
                            if text.len() > max_text {
                                tracing::debug!(bytes = text.len(), "clipboard text over limit");
                                continue;
                            }
                            let h = sha256(text.as_bytes());
                            if Some(h) == last_synced { continue; }
                            last_synced = Some(h);
                            seq += 1;
                            bus.broadcast(&Message::Clipboard {
                                seq,
                                origin_id: own_id.clone(),
                                entries: vec![ClipEntry::text(&text)],
                            }, None);
                            tracing::info!(seq, bytes = text.len(), "clipboard text broadcast");
                        }
                        LocalClip::Image(png) if controls.sync_images() => {
                            let h = sha256(&png);
                            if Some(h) == last_synced { continue; }
                            last_synced = Some(h);
                            if png.len() <= CLIP_IMAGE_INLINE_MAX {
                                seq += 1;
                                bus.broadcast(&Message::Clipboard {
                                    seq,
                                    origin_id: own_id.clone(),
                                    entries: vec![ClipEntry::png(png.clone())],
                                }, None);
                                tracing::info!(seq, bytes = png.len(), "clipboard image broadcast (inline)");
                            } else {
                                tracing::info!(bytes = png.len(), "clipboard image via blob path");
                                filex.send_clip_image(png).await;
                            }
                        }
                        LocalClip::Image(_) => {} // images disabled
                    }
                }

                Some((peer_id, msg)) = clip_frames.recv() => {
                    let Message::Clipboard { origin_id, entries, .. } = msg else { continue };
                    if controls.clipboard_paused() || origin_id == own_id {
                        continue;
                    }
                    let Some(clip) = &clip else { continue };
                    if let Some(text) = entries.iter().find_map(|e| e.as_text()) {
                        if text.len() > max_text { continue; }
                        let h = sha256(text.as_bytes());
                        if Some(h) == last_synced { continue; }
                        last_synced = Some(h);
                        clip.apply_text(text.to_string());
                        tracing::info!(from = %peer_id, bytes = text.len(), "clipboard text applied");
                    } else if controls.sync_images() {
                        if let Some(png) = entries.iter().find_map(|e| e.as_image()) {
                            let h = sha256(png);
                            if Some(h) == last_synced { continue; }
                            last_synced = Some(h);
                            clip.apply_image(png.to_vec());
                            tracing::info!(from = %peer_id, bytes = png.len(), "clipboard image applied");
                        }
                    }
                }

                else => break,
            }
        }
    });

    handle
}
