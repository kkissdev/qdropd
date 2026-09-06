//! Local clipboard access on a dedicated OS thread.
//!
//! Reads and writes go through `arboard`. On macOS we additionally inspect
//! `NSPasteboard` directly to (a) use `changeCount` as the change signal and
//! (b) skip items marked concealed / transient (password managers), which
//! `arboard` cannot see.

use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use qdrop_core::crypto::sha256;
use tokio::sync::mpsc as tok_mpsc;

/// Something the user copied locally.
#[derive(Debug)]
pub enum LocalClip {
    Text(String),
    /// PNG bytes.
    Image(Vec<u8>),
}

/// A remote update to apply locally, or a read request.
pub enum ClipCommand {
    ApplyText(String),
    ApplyImage(Vec<u8>),
    /// Reply with the clipboard's current text (`qdrop paste`).
    GetText(std_mpsc::Sender<Option<String>>),
}

pub struct ClipboardHandle {
    cmd_tx: std_mpsc::Sender<ClipCommand>,
}

impl ClipboardHandle {
    pub fn apply_text(&self, text: String) {
        let _ = self.cmd_tx.send(ClipCommand::ApplyText(text));
    }
    pub fn apply_image(&self, png: Vec<u8>) {
        let _ = self.cmd_tx.send(ClipCommand::ApplyImage(png));
    }
    /// Read the current clipboard text (blocking, with a short timeout).
    pub fn get_text(&self) -> Option<String> {
        let (tx, rx) = std_mpsc::channel();
        self.cmd_tx.send(ClipCommand::GetText(tx)).ok()?;
        rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }
}

/// Start the clipboard thread. Returns `None` when there is no usable
/// clipboard (headless server).
pub fn start(
    poll_interval: Duration,
    sync_images: impl Fn() -> bool + Send + 'static,
    local_changes: tok_mpsc::Sender<LocalClip>,
) -> Option<ClipboardHandle> {
    let mut backend = match Backend::new() {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("clipboard unavailable, sync disabled: {e}");
            return None;
        }
    };
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<ClipCommand>();

    std::thread::Builder::new()
        .name("qdrop-clipboard".into())
        .spawn(move || {
            let images = &sync_images;
            let mut last_token = backend.snapshot(images()).map(|s| s.token);
            let mut suppress: Option<[u8; 32]> = None;

            loop {
                match cmd_rx.recv_timeout(poll_interval) {
                    Ok(ClipCommand::ApplyText(text)) => {
                        suppress = Some(sha256(text.as_bytes()));
                        if let Err(e) = backend.set_text(&text) {
                            tracing::warn!("set clipboard text failed: {e}");
                        }
                        last_token = backend.snapshot(images()).map(|s| s.token);
                    }
                    Ok(ClipCommand::ApplyImage(png)) => {
                        suppress = Some(sha256(&png));
                        if let Err(e) = backend.set_image_png(&png) {
                            tracing::warn!("set clipboard image failed: {e}");
                        }
                        last_token = backend.snapshot(images()).map(|s| s.token);
                    }
                    Ok(ClipCommand::GetText(reply)) => {
                        let _ = reply.send(
                            backend
                                .snapshot(false)
                                .and_then(|s| s.text)
                                .filter(|t| !t.is_empty()),
                        );
                    }
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {
                        let Some(snap) = backend.snapshot(images()) else {
                            continue;
                        };
                        if Some(snap.token) == last_token {
                            continue;
                        }
                        last_token = Some(snap.token);
                        if snap.concealed {
                            tracing::debug!("skipping concealed/transient clipboard item");
                            continue;
                        }
                        let clip = if let Some(png) = snap.image_png {
                            LocalClip::Image(png)
                        } else if let Some(text) = snap.text {
                            LocalClip::Text(text)
                        } else {
                            continue;
                        };
                        let h = match &clip {
                            LocalClip::Text(t) => sha256(t.as_bytes()),
                            LocalClip::Image(b) => sha256(b),
                        };
                        if Some(h) == suppress {
                            suppress = None;
                            continue;
                        }
                        if local_changes.blocking_send(clip).is_err() {
                            break;
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

struct Snapshot {
    /// Monotonic-ish change signal.
    token: i64,
    concealed: bool,
    text: Option<String>,
    image_png: Option<Vec<u8>>,
}

struct Backend {
    clipboard: arboard::Clipboard,
}

impl Backend {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            clipboard: arboard::Clipboard::new()?,
        })
    }

    fn set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.clipboard.set_text(text.to_string())?;
        Ok(())
    }

    fn set_image_png(&mut self, png: &[u8]) -> anyhow::Result<()> {
        let img = image::load_from_memory(png)?.to_rgba8();
        let (w, h) = img.dimensions();
        self.clipboard.set_image(arboard::ImageData {
            width: w as usize,
            height: h as usize,
            bytes: std::borrow::Cow::Owned(img.into_raw()),
        })?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn snapshot(&mut self, sync_images: bool) -> Option<Snapshot> {
        let text = self.clipboard.get_text().ok().filter(|t| !t.is_empty());
        let image_png = if sync_images {
            self.clipboard.get_image().ok().and_then(rgba_to_png)
        } else {
            None
        };
        // No changeCount on this platform; hash the contents.
        let mut h = sha2::Sha256::new();
        use sha2::Digest;
        if let Some(t) = &text {
            h.update(b"t");
            h.update(t.as_bytes());
        }
        if let Some(p) = &image_png {
            h.update(b"i");
            h.update(p);
        }
        let digest: [u8; 32] = h.finalize().into();
        let token = i64::from_le_bytes(digest[..8].try_into().unwrap());
        Some(Snapshot {
            token,
            concealed: linux_looks_concealed(),
            text,
            image_png,
        })
    }

    #[cfg(target_os = "macos")]
    fn snapshot(&mut self, sync_images: bool) -> Option<Snapshot> {
        macos::snapshot(sync_images)
    }
}

#[cfg(not(target_os = "macos"))]
fn rgba_to_png(img: arboard::ImageData) -> Option<Vec<u8>> {
    let rgba =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// Best-effort: some Wayland password managers expose a hint type. If
/// `wl-paste` is present, check the offered MIME types.
#[cfg(not(target_os = "macos"))]
fn linux_looks_concealed() -> bool {
    let out = std::process::Command::new("wl-paste")
        .args(["--list-types"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let types = String::from_utf8_lossy(&o.stdout).to_lowercase();
            types.contains("x-kde-passwordmanagerhint")
                || types.contains("password")
                || types.contains("concealed")
        }
        _ => false,
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Snapshot;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
    };
    use objc2_foundation::NSString;

    const CONCEALED_TYPES: &[&str] = &[
        "org.nspasteboard.ConcealedType",
        "org.nspasteboard.TransientType",
        "org.nspasteboard.AutoGeneratedType",
    ];

    pub fn snapshot(sync_images: bool) -> Option<Snapshot> {
        let pb = NSPasteboard::generalPasteboard();
        let token = pb.changeCount() as i64;

        let mut concealed = false;
        if let Some(types) = pb.types() {
            for t in types.iter() {
                let s = t.to_string();
                if CONCEALED_TYPES.iter().any(|c| s == *c) {
                    concealed = true;
                    break;
                }
            }
        }
        if concealed {
            return Some(Snapshot {
                token,
                concealed: true,
                text: None,
                image_png: None,
            });
        }

        let image_png = if sync_images { read_png(&pb) } else { None };
        let text = if image_png.is_none() {
            pb.stringForType(unsafe { NSPasteboardTypeString })
                .map(|s| s.to_string())
                .filter(|t| !t.is_empty())
        } else {
            None
        };

        Some(Snapshot {
            token,
            concealed: false,
            text,
            image_png,
        })
    }

    fn read_png(pb: &NSPasteboard) -> Option<Vec<u8>> {
        if let Some(data) = pb.dataForType(unsafe { NSPasteboardTypePNG }) {
            return Some(data.to_vec());
        }
        // Fall back to TIFF and transcode.
        let tiff = pb.dataForType(unsafe { NSPasteboardTypeTIFF })?;
        let bytes = tiff.to_vec();
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Tiff).ok()?;
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(out)
    }

    // Silence unused import warning when the `NSString` re-export isn't needed.
    #[allow(dead_code)]
    fn _use(_: &NSString) {}
}
