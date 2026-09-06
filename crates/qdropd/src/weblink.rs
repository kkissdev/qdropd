//! `qdrop open <url>` — push a URL to a peer's default browser.
//!
//! The scheme allowlist ([`qdrop_core::proto::is_allowed_url`]) is enforced on
//! both the sending and receiving side, and the URL is passed to the launcher
//! as a single argv element — never through a shell.

use std::process::Stdio;

use qdrop_core::proto::{is_allowed_url, Message};
use qdrop_core::ConfirmPolicy;

use crate::bus::PeerBus;
use crate::filexfer::{needs_confirm, peer_label};
use crate::notify::notify;

/// Handle an inbound `OpenUrl` from `peer_id`.
pub fn handle_inbound(peer_id: &str, url: String, confirm: ConfirmPolicy) {
    if !is_allowed_url(&url) {
        tracing::warn!(peer = %peer_id, url = %url, "refused OpenUrl: scheme not allowed");
        return;
    }
    if needs_confirm(confirm, peer_id) {
        tracing::info!(peer = %peer_id, url = %url, "holding OpenUrl: peer not authorized");
        notify(
            "Link from a device",
            &format!("{} wants to open: {url}", peer_label(peer_id)),
        );
        return;
    }
    let launcher = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(launcher)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => {
            tracing::info!(peer = %peer_id, url = %url, "opened URL from peer");
            notify(
                "Opened a link",
                &format!("From {}: {url}", peer_label(peer_id)),
            );
        }
        Err(e) => tracing::warn!("failed to launch {launcher}: {e}"),
    }
}

/// Send `url` to `to` (a peer name) or every connected peer. Returns the number
/// of peers it was dispatched to, or an error string.
pub fn dispatch(bus: &PeerBus, url: &str, to: Option<&str>) -> Result<usize, String> {
    if !is_allowed_url(url) {
        return Err(format!(
            "URL scheme not allowed (permitted: {})",
            qdrop_core::proto::URL_SCHEME_ALLOWLIST.join(", ")
        ));
    }
    let targets: Vec<String> = match to {
        Some(name) if bus.sender(name).is_some() => vec![name.to_string()],
        Some(name) => match resolve(name) {
            Some(id) => vec![id],
            None => return Err(format!("no such paired peer: {name}")),
        },
        None => bus.connected_ids(),
    };
    if targets.is_empty() {
        return Err("no connected peers".into());
    }
    let mut n = 0;
    for id in targets {
        if bus.send_to(
            &id,
            Message::OpenUrl {
                url: url.to_string(),
            },
        ) {
            n += 1;
        }
    }
    Ok(n)
}

fn resolve(name: &str) -> Option<String> {
    let peers = qdrop_core::Peers::load().ok()?;
    peers
        .find(name)
        .or_else(|| peers.find_by_id(name))
        .map(|p| p.device_id.clone())
}
