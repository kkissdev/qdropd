//! `tracing` setup shared by both binaries.

use std::io::IsTerminal;

use anyhow::Result;
use tracing_subscriber::{prelude::*, EnvFilter};

/// Initialize the global tracing subscriber.
///
/// Precedence for the log filter: `RUST_LOG` env var, else `debug` when
/// `verbose` is set, else `fallback` (from config). Logs go to stderr so
/// stdout stays clean for machine-readable CLI output; on a tty they are
/// colorized, otherwise plain (suitable for `launchd` / `journald` capture).
pub fn init(verbose: bool, fallback: &str) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let directive = if verbose { "debug" } else { fallback };
        EnvFilter::new(directive)
    });

    let stderr = std::io::stderr();
    let ansi = stderr.is_terminal();
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(ansi)
        .with_target(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("initializing tracing: {e}"))
}
