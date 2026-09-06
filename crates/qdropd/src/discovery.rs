//! mDNS service advertisement and browsing for `_qdrop._tcp.local.`.
//!
//! We register one service instance keyed by our device id and browse for
//! others, filtering ourselves out. Resolved peers and disappearances are
//! forwarded to the transport manager as [`DiscoveryEvent`]s.

use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, Result};
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use qdrop_core::SERVICE_TYPE;
use tokio::sync::mpsc;

/// TXT keys we publish.
const TXT_DEVICE_ID: &str = "id";
const TXT_DEVICE_NAME: &str = "name";
/// Key fingerprint — equal to the device id until pairing (M2) adds real keys.
const TXT_FINGERPRINT: &str = "fp";

/// A peer found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub device_name: String,
    pub fingerprint: String,
    pub addrs: Vec<SocketAddr>,
}

/// Discovery-layer events fed to the transport manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// A peer appeared or its address set changed.
    Found(DiscoveredPeer),
    /// A peer's service record went away.
    Lost { device_id: String },
}

/// Handle that keeps the mDNS registration alive. Drop or call
/// [`Discovery::shutdown`] to unregister.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Discovery {
    /// Cleanly unregister and stop the mDNS daemon.
    pub fn shutdown(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv();
        }
        let _ = self.daemon.shutdown();
    }
}

/// Start advertising and browsing. Returns the keep-alive handle and a
/// receiver of discovery events. `advertise_port` is the TCP port the
/// transport actually bound.
pub fn start(
    device_id: &str,
    device_name: &str,
    advertise_port: u16,
    tx: mpsc::Sender<DiscoveryEvent>,
) -> Result<Discovery> {
    let daemon = ServiceDaemon::new().context("starting mDNS daemon")?;

    let mut txt = HashMap::new();
    txt.insert(TXT_DEVICE_ID.to_string(), device_id.to_string());
    txt.insert(TXT_DEVICE_NAME.to_string(), device_name.to_string());
    txt.insert(TXT_FINGERPRINT.to_string(), device_id.to_string());

    let host = format!("{device_id}.local.");
    let service = ServiceInfo::new(SERVICE_TYPE, device_id, &host, "", advertise_port, txt)
        .context("building service info")?
        .enable_addr_auto();
    let fullname = service.get_fullname().to_string();
    daemon
        .register(service)
        .context("registering mDNS service")?;

    let browse_rx = daemon
        .browse(SERVICE_TYPE)
        .context("starting mDNS browse")?;

    let own_id = device_id.to_string();

    tokio::spawn(async move {
        loop {
            let event = match browse_rx.recv_async().await {
                Ok(e) => e,
                Err(_) => break, // daemon shut down
            };
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    if let Some(peer) = to_peer(&info, &own_id) {
                        tracing::debug!(
                            device_id = %peer.device_id,
                            name = %peer.device_name,
                            addrs = ?peer.addrs,
                            "mDNS resolved peer"
                        );
                        if tx.send(DiscoveryEvent::Found(peer)).await.is_err() {
                            break;
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_ty, fullname) => {
                    if let Some(id) = instance_of(&fullname) {
                        if id != own_id {
                            tracing::debug!(device_id = %id, "mDNS peer removed");
                            if tx
                                .send(DiscoveryEvent::Lost { device_id: id })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        tracing::debug!("discovery browse loop ended");
    });

    Ok(Discovery { daemon, fullname })
}

/// Extract our peer record from a resolved service, or `None` if it is us or
/// is missing required TXT data / addresses.
fn to_peer(info: &ResolvedService, own_id: &str) -> Option<DiscoveredPeer> {
    let txt = &info.txt_properties;
    let device_id = txt.get_property_val_str(TXT_DEVICE_ID)?.to_string();
    if device_id == own_id {
        return None;
    }
    let device_name = txt
        .get_property_val_str(TXT_DEVICE_NAME)
        .unwrap_or(&device_id)
        .to_string();
    let fingerprint = txt
        .get_property_val_str(TXT_FINGERPRINT)
        .unwrap_or(&device_id)
        .to_string();

    let port = info.port;
    let addrs: Vec<SocketAddr> = info
        .addresses
        .iter()
        .map(|ip| SocketAddr::new(ip.to_ip_addr(), port))
        .collect();
    if addrs.is_empty() {
        return None;
    }
    Some(DiscoveredPeer {
        device_id,
        device_name,
        fingerprint,
        addrs,
    })
}

/// The instance label (our device id) from a service fullname like
/// `deadbeef._qdrop._tcp.local.`.
fn instance_of(fullname: &str) -> Option<String> {
    fullname
        .strip_suffix(&format!(".{SERVICE_TYPE}"))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_parsing() {
        assert_eq!(
            instance_of("deadbeef._qdrop._tcp.local.").as_deref(),
            Some("deadbeef")
        );
        assert_eq!(instance_of("something-else.local.").as_deref(), None);
    }
}
