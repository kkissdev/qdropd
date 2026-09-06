//! CLI surface. Every subcommand from the design is present; M0 stubs the
//! behavior and returns a clear "not implemented yet" error pointing at the
//! milestone that fills it in.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "qdrop",
    version,
    about = "Peer-to-peer clipboard, file, and link bridge for your own devices.",
    propagate_version = true
)]
pub struct Cli {
    /// Increase log verbosity (debug). Overridden by `RUST_LOG`.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Pair with another device, or manage existing pairings.
    Pair(PairArgs),
    /// List paired peers and their status.
    Peers(PeersArgs),
    /// Send one or more files to a peer.
    Send(SendArgs),
    /// Open a URL on a peer.
    Open(OpenArgs),
    /// Control clipboard synchronization.
    Clip(ClipArgs),
    /// Run the qdrop daemon in the foreground.
    Daemon(DaemonArgs),
}

#[derive(Debug, clap::Args)]
pub struct PairArgs {
    /// Peer name or IP to pair with. Omit to enter discoverable mode (side A).
    pub target: Option<String>,
    /// Remove an existing pairing by name.
    #[arg(long, value_name = "NAME")]
    pub remove: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct PeersArgs {
    /// Emit JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct SendArgs {
    /// Files to send.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
    /// Target peer name. Defaults to all paired peers (or the sole peer).
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct OpenArgs {
    /// URL to open on the peer.
    pub url: String,
    /// Target peer name.
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ClipArgs {
    #[command(subcommand)]
    pub action: ClipAction,
}

#[derive(Debug, Subcommand)]
pub enum ClipAction {
    /// Pause clipboard sync.
    Pause,
    /// Resume clipboard sync.
    Resume,
    /// Show clipboard sync status.
    Status,
}

#[derive(Debug, clap::Args)]
pub struct DaemonArgs {
    /// Override the listen port from config.
    #[arg(long)]
    pub port: Option<u16>,
}
