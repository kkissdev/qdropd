//! Wire protocol: message types and capability negotiation.
//!
//! Framing lives in [`crate::frame`]. Transport (who dials whom, keepalive,
//! reconnect) lives in the daemon. This module is just the vocabulary.

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change to the message set or framing.
pub const PROTOCOL_VERSION: u32 = 1;

/// Capabilities a peer advertises in its [`Hello`]. The effective set for a
/// connection is the intersection of both sides' caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Text clipboard sync (M3).
    #[serde(default)]
    pub clipboard_text: bool,
    /// Image clipboard sync (M6).
    #[serde(default)]
    pub clipboard_images: bool,
    /// File transfer (M4).
    #[serde(default)]
    pub file_send: bool,
    /// Remote URL open (M5).
    #[serde(default)]
    pub open_url: bool,
    /// Pre-authorization exchange (M10).
    #[serde(default)]
    pub auth: bool,
}

impl Caps {
    /// No capabilities.
    pub const NONE: Caps = Caps {
        clipboard_text: false,
        clipboard_images: false,
        file_send: false,
        open_url: false,
        auth: false,
    };

    /// Everything this build knows how to do.
    pub const ALL: Caps = Caps {
        clipboard_text: true,
        clipboard_images: true,
        file_send: true,
        open_url: true,
        auth: true,
    };

    /// The capabilities both sides share.
    #[must_use]
    pub fn intersect(self, other: Caps) -> Caps {
        Caps {
            clipboard_text: self.clipboard_text && other.clipboard_text,
            clipboard_images: self.clipboard_images && other.clipboard_images,
            file_send: self.file_send && other.file_send,
            open_url: self.open_url && other.open_url,
            auth: self.auth && other.auth,
        }
    }
}

/// Identifying facts a device shares during the M10 authorization exchange.
/// None of it is a security boundary — the pinned key is. It is a
/// human-readable label plus a soft "same physical machine" check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub hostname: String,
    /// MAC addresses of non-loopback interfaces, lowercase `aa:bb:...`.
    pub macs: Vec<String>,
    /// `std::env::consts::OS` of the peer.
    pub os: String,
}

/// First frame each side sends after the TCP connection opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub device_id: String,
    pub device_name: String,
    pub caps: Caps,
}

/// MIME type for the plain-text clipboard entry (M3).
pub const MIME_TEXT: &str = "text/plain;charset=utf-8";
/// MIME type for clipboard images (M6). PNG on the wire regardless of source.
pub const MIME_PNG: &str = "image/png";
/// Clipboard images at or below this size are sent inline in a `Clipboard`
/// frame; larger ones go through the blob path.
pub const CLIP_IMAGE_INLINE_MAX: usize = 256 * 1024;

/// What a blob transfer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobPurpose {
    /// A file, saved to the downloads directory (M4).
    #[default]
    File,
    /// A clipboard image, applied to the local clipboard (M6).
    ClipboardImage,
}

/// One typed payload in a clipboard update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub mime: String,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

impl ClipEntry {
    pub fn text(s: &str) -> Self {
        Self {
            mime: MIME_TEXT.to_string(),
            data: s.as_bytes().to_vec(),
        }
    }

    /// A PNG image entry.
    pub fn png(bytes: Vec<u8>) -> Self {
        Self {
            mime: MIME_PNG.to_string(),
            data: bytes,
        }
    }

    /// The entry's text, if it is a text MIME type and valid UTF-8.
    pub fn as_text(&self) -> Option<&str> {
        if self.mime.starts_with("text/") {
            std::str::from_utf8(&self.data).ok()
        } else {
            None
        }
    }

    /// The entry's bytes, if it is an image.
    pub fn as_image(&self) -> Option<&[u8]> {
        self.mime.starts_with("image/").then_some(&self.data[..])
    }
}

/// Every frame on the wire.
///
/// `#[serde(tag = "t")]` gives a self-describing, forward-compatible encoding:
/// an old peer that receives an unknown variant fails to decode that one
/// frame rather than desyncing the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Message {
    Hello(Hello),
    Ping {
        seq: u64,
    },
    Pong {
        seq: u64,
    },
    /// A clipboard update. `origin_id` is the device the copy happened on and
    /// `seq` is monotonic per origin — together they let receivers drop echoes.
    Clipboard {
        seq: u64,
        origin_id: String,
        entries: Vec<ClipEntry>,
    },

    /// Begin a blob transfer. `name` is a bare filename (no path); the
    /// receiver sanitises it again regardless.
    BlobStart {
        id: u64,
        name: String,
        size: u64,
        /// What the blob is for (defaults to a file download).
        #[serde(default)]
        purpose: BlobPurpose,
    },
    /// One chunk of a blob (~64 KiB). Chunks for a given `id` arrive in order.
    BlobChunk {
        id: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// End a blob transfer; `sha256` is the digest of the whole payload.
    BlobEnd {
        id: u64,
        sha256: [u8; 32],
    },
    /// Receiver's verdict on a completed (or rejected) transfer.
    BlobAck {
        id: u64,
        ok: bool,
        detail: String,
    },

    /// Ask the peer to open a URL in its default handler (M5).
    OpenUrl {
        url: String,
    },

    /// Ask the peer to pre-authorize us as an unattended sender (M10).
    /// `info` is the requester's own details; `mutual` asks the peer to
    /// authorize the requester back in the same exchange.
    AuthRequest {
        mutual: bool,
        info: DeviceInfo,
    },
    /// Reply to [`Message::AuthRequest`] with the replier's details.
    AuthReply {
        ok: bool,
        info: DeviceInfo,
    },
}

/// Blob chunk size on the wire.
pub const BLOB_CHUNK: usize = 64 * 1024;

/// URL schemes the peer will actually open. Everything else — `file:`,
/// `javascript:`, `data:`, shell-ish strings — is refused on both ends.
pub const URL_SCHEME_ALLOWLIST: &[&str] = &["http", "https", "mailto"];

/// Whether `url` is safe to hand to `open` / `xdg-open` on the peer.
pub fn is_allowed_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.len() > 4096 {
        return false;
    }
    // No control characters, whitespace, or quotes — even though we never pass
    // the URL through a shell.
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\'' || c == '`')
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    if !URL_SCHEME_ALLOWLIST
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
    {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => rest.starts_with("//") && rest.len() > 2,
        "mailto" => rest.contains('@'),
        _ => false,
    }
}

/// Validate an incoming blob filename. Returns the safe basename, or `None` if
/// it tries to escape the download directory.
pub fn safe_blob_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    // Must be exactly its own final component: no separators, no traversal,
    // not absolute, no drive/UNC prefixes.
    let p = std::path::Path::new(name);
    let mut comps = p.components();
    let only = comps.next();
    if comps.next().is_some() {
        return None;
    }
    match only {
        Some(std::path::Component::Normal(os)) => {
            let s = os.to_str()?;
            if s == ".." || s.contains('/') || s.contains('\\') {
                None
            } else {
                Some(s.to_string())
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_intersection() {
        let a = Caps {
            clipboard_text: true,
            file_send: true,
            ..Caps::NONE
        };
        let b = Caps {
            clipboard_text: true,
            open_url: true,
            ..Caps::NONE
        };
        let got = a.intersect(b);
        assert!(got.clipboard_text);
        assert!(!got.file_send);
        assert!(!got.open_url);
    }

    #[test]
    fn message_roundtrips_msgpack() {
        for msg in [
            Message::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                device_id: "abc".into(),
                device_name: "thor".into(),
                caps: Caps::ALL,
            }),
            Message::Ping { seq: 7 },
            Message::Pong { seq: 7 },
            Message::Clipboard {
                seq: 42,
                origin_id: "dev".into(),
                entries: vec![ClipEntry::text("hello \u{1f600}")],
            },
        ] {
            let bytes = rmp_serde::to_vec_named(&msg).unwrap();
            let back: Message = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(msg, back);
        }
    }

    #[test]
    fn blob_name_sanitisation() {
        assert_eq!(safe_blob_name("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(safe_blob_name("  a b.txt ").as_deref(), Some("a b.txt"));
        for bad in [
            "../../etc/passwd",
            "/etc/passwd",
            "a/b.txt",
            "..",
            ".",
            "",
            "foo/../bar",
            r"..\windows",
        ] {
            assert_eq!(safe_blob_name(bad), None, "{bad:?} should be rejected");
        }
    }

    #[test]
    fn url_allowlist() {
        for ok in [
            "https://example.com",
            "http://192.168.1.5:8080/path?q=1",
            "HTTPS://EXAMPLE.COM",
            "mailto:a@b.com",
        ] {
            assert!(is_allowed_url(ok), "{ok:?} should be allowed");
        }
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<script>",
            "http:/nohost",
            "https://exa mple.com",
            "https://example.com/\"; rm -rf /",
            "ftp://example.com",
            "",
            "not a url",
            "vbscript:msgbox",
        ] {
            assert!(!is_allowed_url(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn clip_entry_text_helpers() {
        let e = ClipEntry::text("hi");
        assert_eq!(e.as_text(), Some("hi"));
        let bin = ClipEntry {
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        };
        assert_eq!(bin.as_text(), None);
    }
}
