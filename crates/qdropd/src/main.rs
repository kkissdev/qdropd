//! `qdropd` — the long-running daemon.
//!
//! M0 brings up the runtime, loads config, initializes logging, and waits
//! for a shutdown signal. Discovery and transport land in M1.

use anyhow::{Context, Result};
use clap::Parser;
use qdrop_core::{Config, Peers};

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

    let port = args.port.unwrap_or(config.port);
    let peers = Peers::load()?;

    tracing::info!(
        version = qdrop_core::VERSION,
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
        // M1 will spawn discovery + transport tasks here.
        tracing::info!("no work to do yet (discovery arrives in M1); waiting for shutdown signal");
        wait_for_shutdown().await;
        tracing::info!("shutdown signal received, stopping");
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
