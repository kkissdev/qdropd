//! CLI surface for the `qdrop` client.

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

    /// Suppress progress/status output (errors still print).
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Pair with another device, or manage existing pairings.
    Pair(PairArgs),
    /// List paired peers and their status.
    Peers(PeersArgs),
    /// Send one or more files to a peer (use `-` for stdin).
    Send(SendArgs),
    /// Wait for an incoming file; print its path or stream it to stdout.
    Recv(RecvArgs),
    /// Print a peer's shared clipboard text to stdout.
    Paste,
    /// Read stdin and set it as the shared clipboard.
    Copy,
    /// Open a URL on a peer.
    Open(OpenArgs),
    /// Control clipboard synchronization.
    Clip(ClipArgs),
    /// Pre-authorize a paired peer to send here unattended.
    Auth(AuthArgs),
    /// Show live daemon status (peers, clipboard).
    Status(StatusArgs),
    /// Diagnose why sync isn't working.
    Doctor(DoctorArgs),
    /// Run the qdrop daemon in the foreground.
    Daemon(DaemonArgs),
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Emit the checks as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Pre-authorize a peer as an unattended sender. With no argument, lists which
/// peers are authorized.
#[derive(Debug, clap::Args)]
pub struct AuthArgs {
    /// Peer name or device id to authorize (must be paired and connected).
    pub name: Option<String>,
    /// Revoke authorization for this peer.
    #[arg(long, value_name = "NAME")]
    pub remove: Option<String>,
    /// Also ask the peer to authorize this machine back.
    #[arg(long)]
    pub mutual: bool,
}

#[derive(Debug, clap::Args)]
pub struct StatusArgs {
    /// Emit the raw JSON from the daemon.
    #[arg(long)]
    pub json: bool,
    /// Emit a single line of waybar JSON (`text` / `tooltip` / `class`).
    #[arg(long)]
    pub waybar: bool,
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
    /// Files to send. A single `-` reads the file from stdin.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
    /// Target peer name. Defaults to all connected peers.
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
    /// Filename to use for stdin (`-`). Default `stdin-<timestamp>.bin`.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct RecvArgs {
    /// Write the file's contents to stdout instead of leaving it on disk.
    #[arg(long)]
    pub stdout: bool,
    /// With `--stdout`, keep the file in the download folder too.
    #[arg(long)]
    pub keep: bool,
    /// Give up after this many seconds (default 300).
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,
}

#[derive(Debug, clap::Args)]
pub struct OpenArgs {
    /// URL to open on the peer.
    pub url: String,
    /// Target peer name.
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
}

/// Control clipboard sync. With no flag, prints status.
#[derive(Debug, clap::Args)]
pub struct ClipArgs {
    /// Pause clipboard sync.
    #[arg(long)]
    pub pause: bool,
    /// Resume clipboard sync.
    #[arg(long)]
    pub resume: bool,
    /// Flip between paused and active.
    #[arg(long)]
    pub toggle: bool,
    /// Show clipboard sync status (the default).
    #[arg(long)]
    pub status: bool,
    /// List recent clipboard entries.
    #[arg(long)]
    pub history: bool,
    /// Put history entry N back on the clipboard.
    #[arg(long, value_name = "N")]
    pub restore: Option<usize>,
    /// Send history entry N to a peer.
    #[arg(long, value_name = "N")]
    pub send: Option<usize>,
    /// Target peer for `--send` (default: all connected).
    #[arg(long, value_name = "NAME")]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum ClipAction {
    Pause,
    Resume,
    Toggle,
    Status,
    History,
    Restore(usize),
    Send(usize),
}

impl ClipArgs {
    pub fn action(&self) -> ClipAction {
        if self.pause {
            ClipAction::Pause
        } else if self.resume {
            ClipAction::Resume
        } else if self.toggle {
            ClipAction::Toggle
        } else if self.history {
            ClipAction::History
        } else if let Some(n) = self.restore {
            ClipAction::Restore(n)
        } else if let Some(n) = self.send {
            ClipAction::Send(n)
        } else {
            ClipAction::Status
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct DaemonArgs {
    /// Override the listen port from config.
    #[arg(long)]
    pub port: Option<u16>,
}
