//! `qdrop doctor` — diagnose why sync isn't working (M16).

use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;

use crate::{Config, Peers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Ok,
            detail: detail.into(),
            fix: None,
        }
    }
    fn warn(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
    fn fail(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Run every check. Requires a tokio runtime.
pub async fn run() -> Vec<Check> {
    let mut out = Vec::new();

    // --- config -----------------------------------------------------------
    let config = match Config::load() {
        Ok(c) => {
            out.push(Check::ok(
                "config",
                format!("device \"{}\", port {}", c.device_name, c.port),
            ));
            c
        }
        Err(e) => {
            out.push(Check::fail(
                "config",
                format!("{e:#}"),
                "fix the syntax error in ~/.config/qdrop/config.toml",
            ));
            Config::default()
        }
    };

    // --- identity --------------------------------------------------------
    match crate::paths::identity_file() {
        Ok(p) if p.exists() => match crate::Identity::load_or_create_at(&p) {
            Ok(id) => out.push(Check::ok(
                "identity",
                format!("fingerprint {}", id.fingerprint()),
            )),
            Err(e) => out.push(Check::fail(
                "identity",
                format!("{e:#}"),
                "the key file is corrupt — stop the daemon, delete identity.pem, re-pair",
            )),
        },
        Ok(_) => out.push(Check::warn(
            "identity",
            "no key yet",
            "it is created on the first `qdropd` start",
        )),
        Err(e) => out.push(Check::fail(
            "identity",
            format!("{e:#}"),
            "check $HOME / $XDG_CONFIG_HOME",
        )),
    }

    // --- daemon (control socket) ----------------------------------------
    let status = crate::control::request("status").await.ok();
    let daemon_up = status.is_some();
    match &status {
        Some(s) => out.push(Check::ok(
            "daemon",
            format!(
                "running, version {}",
                s.get("version").and_then(|v| v.as_str()).unwrap_or("?")
            ),
        )),
        None => out.push(Check::fail(
            "daemon",
            "not reachable on the control socket",
            "start it: `qdropd` (or `systemctl --user start qdropd` / launchd)",
        )),
    }

    // --- listen port ---------------------------------------------------
    let daemon_port = status
        .as_ref()
        .and_then(|s| s.get("port"))
        .and_then(|v| v.as_u64())
        .map(|p| p as u16);
    let check_port = daemon_port.unwrap_or(config.port);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], check_port));
    match std::net::TcpListener::bind(addr) {
        _ if daemon_port.is_some() => out.push(Check::ok(
            "port",
            format!("daemon is listening on TCP {check_port}"),
        )),
        Ok(_) => out.push(Check::ok("port", format!("TCP {check_port} is bindable"))),
        Err(e) => out.push(Check::fail(
            "port",
            format!("cannot bind TCP {check_port}: {e}"),
            "another process holds the port — change `port` in config.toml",
        )),
    }

    // --- mDNS + peers -------------------------------------------------
    let (services, own_id) = browse_mdns().await;
    if services.is_empty() && daemon_up {
        out.push(Check::warn(
            "mDNS",
            "browsed for _qdrop._tcp and saw nothing (not even this daemon)",
            "multicast may be blocked — check the firewall / that Wi-Fi client isolation is off",
        ));
    } else if services.is_empty() {
        out.push(Check::warn(
            "mDNS",
            "no _qdrop._tcp services on the network",
            "start `qdropd` on this and the other machine",
        ));
    } else {
        out.push(Check::ok(
            "mDNS",
            format!("{} _qdrop._tcp service(s) visible", services.len()),
        ));
    }

    let online: Vec<String> = status
        .as_ref()
        .and_then(|s| s.get("peers"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|p| p.get("online").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|p| {
                    p.get("device_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    for peer in Peers::load().unwrap_or_default().peers {
        if online.contains(&peer.device_id) {
            out.push(Check::ok(&format!("peer:{}", peer.name), "connected"));
        } else if services.iter().any(|id| id == &peer.device_id) {
            out.push(Check::warn(
                &format!("peer:{}", peer.name),
                "discovered but not connected",
                "check both daemons' logs; keys may not match (re-pair) or clocks may be skewed",
            ));
        } else if Some(&peer.device_id) == own_id.as_ref() {
            // shouldn't happen
        } else {
            out.push(Check::warn(
                &format!("peer:{}", peer.name),
                "not seen on the network",
                "is its daemon running and on the same LAN? (or set a static endpoint — M11)",
            ));
        }
    }

    // --- downloads dir ---------------------------------------------
    match crate::paths::downloads_dir() {
        Ok(dir) => {
            let probe = dir.join(".qdrop-doctor-probe");
            match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&probe, b"x")) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&probe);
                    out.push(Check::ok(
                        "downloads",
                        format!("{} is writable", dir.display()),
                    ));
                }
                Err(e) => out.push(Check::fail(
                    "downloads",
                    format!("cannot write to {}: {e}", dir.display()),
                    "fix permissions or set QDROP_DOWNLOAD_DIR",
                )),
            }
        }
        Err(e) => out.push(Check::fail(
            "downloads",
            format!("{e:#}"),
            "set QDROP_DOWNLOAD_DIR",
        )),
    }

    out
}

/// Browse `_qdrop._tcp` for ~2.5s; return the device ids seen and our own id.
async fn browse_mdns() -> (Vec<String>, Option<String>) {
    let own_id = crate::device::load_or_create().ok();
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(_) => return (Vec::new(), own_id),
    };
    let Ok(rx) = daemon.browse(crate::SERVICE_TYPE) else {
        return (Vec::new(), own_id);
    };

    let mut ids = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv_async()).await {
            Ok(Ok(ServiceEvent::ServiceResolved(info))) => {
                if let Some(id) = info.txt_properties.get_property_val_str("id") {
                    if !ids.contains(&id.to_string()) {
                        ids.push(id.to_string());
                    }
                }
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    let _ = daemon.shutdown();
    (ids, own_id)
}
