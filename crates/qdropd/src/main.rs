//! `qdropd` — the long-running daemon.
//!
//! M2: mutually-authenticated TLS 1.3, keys pinned during pairing.
//! M3: bidirectional clipboard **text** sync + a control socket for
//! `qdrop clip --pause/--resume/--status`.

mod auth;
mod backoff;
mod bus;
mod clipboard;
mod connection;
mod control;
mod discovery;
mod filexfer;
mod history;
mod notify;
mod secure;
mod state;
mod sync;
mod transport;
mod weblink;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use qdrop_core::proto::{Caps, Hello, Message, PROTOCOL_VERSION};
use qdrop_core::{Config, Identity, Peers, Roster};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::bus::PeerBus;
use crate::control::{ControlDeps, Controls};
use crate::filexfer::FileXfer;
use crate::transport::{TransportConfig, TransportEvent};

#[derive(Debug, Parser)]
#[command(
    name = "qdropd",
    version,
    about = "The qdrop daemon.",
    propagate_version = true
)]
struct Args {
    /// Increase log verbosity (debug). Overridden by `RUST_LOG`.
    #[arg(short, long)]
    verbose: bool,

    /// Override the listen port from config.
    #[arg(long)]
    port: Option<u16>,

    /// Load config, log the plan, and exit without starting the runtime.
    #[arg(long)]
    check: bool,
}

fn main() -> std::process::ExitCode {
    match real_main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("qdropd: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load()?;
    qdrop_core::logging::init(args.verbose, &config.log_filter)?;

    let device_id = qdrop_core::device::load_or_create()?;
    let identity = Arc::new(Identity::load_or_create().context("loading identity key")?);
    let port = args.port.unwrap_or(config.port);
    let peers = Peers::load()?;

    tracing::info!(
        version = qdrop_core::VERSION,
        device_id = %device_id,
        device_name = %config.device_name,
        fingerprint = %identity.fingerprint(),
        port,
        paired_peers = peers.peers.len(),
        "qdropd starting"
    );

    if args.check {
        tracing::info!("--check: configuration OK, exiting");
        return Ok(());
    }

    if peers.is_empty() {
        tracing::warn!(
            "no devices paired yet — run `qdrop pair` on this machine and \
             `qdrop pair {}` on the other one",
            config.device_name
        );
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(async move {
        let local = Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: device_id.clone(),
            device_name: config.device_name.clone(),
            caps: Caps::ALL,
        };

        let roster = Roster::from_peers(&peers);
        let server_config = qdrop_core::tls::server_config(&identity, roster.clone())
            .context("building TLS server config")?;

        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
            .await
            .with_context(|| format!("binding TCP listener on port {port}"))?;
        let bound_port = listener.local_addr()?.port();

        let (discovery_tx, discovery_rx) =
            tokio::sync::mpsc::channel::<discovery::DiscoveryEvent>(64);

        // Seed the dialer with each paired peer's last-known address so it can
        // connect straight away after a network transition, before mDNS
        // re-resolves.
        {
            let cached = qdrop_core::state::DaemonState::load().unwrap_or_default();
            for p in &peers.peers {
                if let Some(addr) = cached
                    .last_addr
                    .get(&p.device_id)
                    .and_then(|s| s.parse::<SocketAddr>().ok())
                {
                    let _ = discovery_tx.try_send(discovery::DiscoveryEvent::Found(
                        discovery::DiscoveredPeer {
                            device_id: p.device_id.clone(),
                            device_name: p.name.clone(),
                            fingerprint: p.public_key.clone(),
                            addrs: vec![addr],
                        },
                    ));
                }
            }
        }

        let discovery = discovery::start(&device_id, &config.device_name, bound_port, discovery_tx)
            .context("starting mDNS discovery")?;

        let (roster_tx, roster_rx) = watch::channel(());
        spawn_roster_reload(roster.clone(), roster_tx);

        let bus = PeerBus::new();
        let controls = Arc::new(Controls::default());
        let (hub_tx, hub_rx) = tokio::sync::mpsc::channel::<(String, Message)>(64);

        let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<TransportEvent>(64);
        let manager = transport::spawn(
            TransportConfig::new(local, identity.clone(), roster, server_config),
            listener,
            discovery_rx,
            roster_rx,
            bus.clone(),
            hub_tx,
            Some(ev_tx),
        );

        let download_dir = qdrop_core::paths::downloads_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("qdrop-downloads"));
        let (clip_img_tx, clip_img_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
        let filex = FileXfer::new(&config, bus.clone(), download_dir, Some(clip_img_tx));
        let auth = auth::AuthManager::new(bus.clone());
        let confirm_policy = config.require_confirm;
        let history = history::History::load(history::dir(), config.clipboard_history);

        // Route inbound application frames to the right subsystem.
        let (clip_tx, clip_rx) = tokio::sync::mpsc::channel::<(String, Message)>(64);
        {
            let filex = filex.clone();
            let auth = auth.clone();
            let mut hub_rx = hub_rx;
            tokio::spawn(async move {
                while let Some((peer, msg)) = hub_rx.recv().await {
                    match msg {
                        Message::Clipboard { .. } => {
                            let _ = clip_tx.send((peer, msg)).await;
                        }
                        Message::BlobStart { .. }
                        | Message::BlobChunk { .. }
                        | Message::BlobEnd { .. }
                        | Message::BlobAck { .. } => filex.handle_inbound(peer, msg).await,
                        Message::OpenUrl { url } => {
                            weblink::handle_inbound(&peer, url, confirm_policy)
                        }
                        Message::AuthRequest { .. } | Message::AuthReply { .. } => {
                            auth.handle_inbound(peer, msg).await
                        }
                        _ => {}
                    }
                }
            });
        }

        let clipboard = sync::spawn(
            &config,
            device_id.clone(),
            bus.clone(),
            filex.clone(),
            history.clone(),
            clip_rx,
            clip_img_rx,
            controls.clone(),
        );
        spawn_config_reload(controls.clone());
        let control = match control::spawn(ControlDeps {
            controls: controls.clone(),
            bus: bus.clone(),
            filex: filex.clone(),
            auth: auth.clone(),
            clipboard,
            history: history.clone(),
            own_id: device_id.clone(),
            port: bound_port,
        }) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!("control socket unavailable: {e:#}");
                None
            }
        };

        tracing::info!(
            port = bound_port,
            "listening; advertising over mDNS (TLS 1.3, pinned keys); clipboard sync active"
        );

        let events = tokio::spawn(state::publish_events(ev_rx, auth.clone()));

        wait_for_shutdown().await;
        tracing::info!("shutdown signal received, stopping");

        manager.abort();
        events.abort();
        if let Some(c) = control {
            c.abort();
        }
        let _ = std::fs::remove_file(control::socket_path().unwrap_or_default());
        discovery.shutdown();
        Ok(())
    })
}

/// Poll `peers.toml` and refresh the shared roster when it changes.
fn spawn_roster_reload(roster: Roster, notify: watch::Sender<()>) {
    tokio::spawn(async move {
        let path = match qdrop_core::paths::peers_file() {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut last = mtime(&path);
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = mtime(&path);
            if now != last {
                last = now;
                match Peers::load_from(&path) {
                    Ok(peers) => {
                        roster.load(&peers);
                        tracing::info!(
                            peers = peers.peers.len(),
                            "roster reloaded from peers.toml"
                        );
                        let _ = notify.send(());
                    }
                    Err(e) => tracing::warn!("reloading peers.toml failed: {e:#}"),
                }
            }
        }
    });
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Poll `config.toml` and push runtime-adjustable settings into `Controls`
/// (currently just `sync_images`), so the toggle takes effect without a
/// restart.
fn spawn_config_reload(controls: Arc<Controls>) {
    tokio::spawn(async move {
        let Ok(path) = qdrop_core::paths::config_file() else {
            return;
        };
        let mut last = mtime(&path);
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let now = mtime(&path);
            if now != last {
                last = now;
                if let Ok(cfg) = Config::load_from(&path) {
                    controls.set_sync_images(cfg.sync_images);
                    tracing::info!(sync_images = cfg.sync_images, "config reloaded");
                }
            }
        }
    });
}

#[cfg(unix)]
async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot listen for SIGTERM: {e}; using Ctrl-C only");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
