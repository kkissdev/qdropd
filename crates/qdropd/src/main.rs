//! `qdropd` — the long-running daemon.
//!
//! M1: advertise + browse `_qdrop._tcp` over mDNS, and hold one plaintext TCP
//! connection per discovered peer (`Hello` handshake + `Ping`/`Pong`
//! keepalive + reconnect). Encryption arrives in M2.

mod backoff;
mod connection;
mod discovery;
mod transport;

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use clap::Parser;
use qdrop_core::proto::{Caps, Hello, PROTOCOL_VERSION};
use qdrop_core::{Config, Peers};
use tokio::net::TcpListener;

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
    let port = args.port.unwrap_or(config.port);
    let peers = Peers::load()?;

    tracing::info!(
        version = qdrop_core::VERSION,
        device_id = %device_id,
        device_name = %config.device_name,
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
            // Advertise everything this build knows; real subsystems land in M3+.
            caps: Caps::ALL,
        };

        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
            .await
            .with_context(|| format!("binding TCP listener on port {port}"))?;
        let bound_port = listener.local_addr()?.port();

        let (discovery, discovery_rx) =
            discovery::start(&device_id, &config.device_name, bound_port)
                .context("starting mDNS discovery")?;

        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<TransportEvent>(64);
        let manager = transport::spawn(
            TransportConfig::new(local),
            listener,
            discovery_rx,
            Some(ev_tx),
        );

        tracing::info!(port = bound_port, "listening; advertising over mDNS");

        let events = tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                match ev {
                    TransportEvent::PeerConnected {
                        device_id,
                        device_name,
                        caps,
                    } => {
                        tracing::info!(peer = %device_id, name = %device_name, caps = ?caps, "peer online");
                    }
                    TransportEvent::PeerDisconnected { device_id, reason } => {
                        tracing::info!(peer = %device_id, %reason, "peer offline");
                    }
                }
            }
        });

        wait_for_shutdown().await;
        tracing::info!("shutdown signal received, stopping");

        manager.abort();
        events.abort();
        discovery.shutdown();
        Ok(())
    })
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
