//! Local clipboard access, isolated on a dedicated OS thread.
//!
//! `arboard`'s handle is neither `Send` nor cheap to recreate, and on macOS
//! pasteboard access is happier off the async runtime's worker threads. So we
//! own it on one thread that:
//!   * polls the clipboard ~4x/second and reports genuine local changes, and
//!   * applies remote clipboard updates on request, remembering the hash it
//!     just wrote so the next poll does not echo it back.

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use qdrop_core::crypto::sha256;
use tokio::sync::mpsc as tok_mpsc;

/// Command sent to the clipboard thread.
pub enum ClipCommand {
    /// Apply this text locally (from a remote peer).
    Apply(String),
}

/// Handle to the clipboard thread.
pub struct ClipboardHandle {
    cmd_tx: std_mpsc::Sender<ClipCommand>,
}

impl ClipboardHandle {
    /// Apply remote text to the local clipboard.
    pub fn apply(&self, text: String) {
        let _ = self.cmd_tx.send(ClipCommand::Apply(text));
    }
}

/// Start the clipboard thread. `local_changes` receives the new text whenever
/// the user copies something locally (echoes of `apply` are filtered out).
/// Returns `None` if there is no usable clipboard (e.g. a headless server).
pub fn start(
    poll_interval: Duration,
    local_changes: tok_mpsc::Sender<String>,
) -> Option<ClipboardHandle> {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("clipboard unavailable, sync disabled: {e}");
            return None;
        }
    };

    let (cmd_tx, cmd_rx) = std_mpsc::channel::<ClipCommand>();

    std::thread::Builder::new()
        .name("qdrop-clipboard".into())
        .spawn(move || {
            // Seed with whatever is on the clipboard now so we do not
            // broadcast the pre-existing contents on startup.
            let mut last_hash = clipboard.get_text().ok().map(|t| sha256(t.as_bytes()));
            let mut suppress: Option<[u8; 32]> = None;

            loop {
                match cmd_rx.recv_timeout(poll_interval) {
                    Ok(ClipCommand::Apply(text)) => {
                        let h = sha256(text.as_bytes());
                        match clipboard.set_text(text) {
                            Ok(()) => {
                                suppress = Some(h);
                                last_hash = Some(h);
                            }
                            Err(e) => tracing::warn!("failed to set clipboard: {e}"),
                        }
                    }
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {
                        let text = match clipboard.get_text() {
                            Ok(t) => t,
                            Err(_) => continue, // empty / non-text / transient
                        };
                        let h = sha256(text.as_bytes());
                        if Some(h) == last_hash {
                            continue;
                        }
                        last_hash = Some(h);
                        if Some(h) == suppress {
                            suppress = None; // consumed the echo of our own write
                            continue;
                        }
                        if local_changes.blocking_send(text).is_err() {
                            break; // async side gone
                        }
                    }
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            tracing::debug!("clipboard thread exiting");
        })
        .ok()?;

    Some(ClipboardHandle { cmd_tx })
}
