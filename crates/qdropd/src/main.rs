//! `qdropd` — the long-running daemon.
//!
//! M2: mutually-authenticated TLS 1.3 with keys pinned during pairing. Only
//! peers in `peers.toml` are dialed or accepted; the roster is reloaded when
//! that file changes, so `qdrop pair` / `qdrop pair --remove` take effect
//! without a restart.

mod backoff;
mod connection;
mod discovery;
mod secure;
mod state;
mod transport;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use qdrop_core::proto::{Caps, Hello, PROTOCOL_VERSION};
use qdrop_core::{Config, Identity, Peers, Roster};
use tokio::net::TcpListener;
use tokio::sync::watch;

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

        let (discovery, discovery_rx) =
            discovery::start(&device_id, &config.device_name, bound_port)
                .context("starting mDNS discovery")?;

        let (roster_tx, roster_rx) = watch::channel(());
        spawn_roster_reload(roster.clone(), roster_tx);

        let (ev_tx, ev_rx) = tokio::sync::mpsc::channel::<TransportEvent>(64);
        let manager = transport::spawn(
            TransportConfig::new(local, identity.clone(), roster, server_config),
            listener,
            discovery_rx,
            roster_rx,
            Some(ev_tx),
        );

        tracing::info!(
            port = bound_port,
            "listening; advertising over mDNS (TLS 1.3, pinned keys)"
        );

        let events = tokio::spawn(state::publish_events(ev_rx));

        wait_for_shutdown().await;
        tracing::info!("shutdown signal received, stopping");

        manager.abort();
        events.abort();
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
