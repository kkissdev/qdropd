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
}

impl Caps {
    /// No capabilities.
    pub const NONE: Caps = Caps {
        clipboard_text: false,
        clipboard_images: false,
        file_send: false,
        open_url: false,
    };

    /// Everything this build knows how to do.
    pub const ALL: Caps = Caps {
        clipboard_text: true,
        clipboard_images: true,
        file_send: true,
        open_url: true,
    };

    /// The capabilities both sides share.
    #[must_use]
    pub fn intersect(self, other: Caps) -> Caps {
        Caps {
            clipboard_text: self.clipboard_text && other.clipboard_text,
            clipboard_images: self.clipboard_images && other.clipboard_images,
            file_send: self.file_send && other.file_send,
            open_url: self.open_url && other.open_url,
        }
    }
}

/// First frame each side sends after the TCP connection opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub device_id: String,
    pub device_name: String,
    pub caps: Caps,
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
    Ping { seq: u64 },
    Pong { seq: u64 },
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
        ] {
            let bytes = rmp_serde::to_vec_named(&msg).unwrap();
            let back: Message = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(msg, back);
        }
    }
}
