//! `qdrop` — the command-line client.

mod cli;

use anyhow::Result;
use clap::Parser;
use qdrop_core::{Config, Peers};

use crate::cli::{Cli, ClipAction, Command};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("qdrop: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    qdrop_core::logging::init(cli.verbose, &config.log_filter)?;

    tracing::debug!(command = ?cli.command, "dispatching subcommand");

    match cli.command {
        Command::Peers(args) => cmd_peers(&args),
        Command::Pair(args) => cmd_pair(&args),
        Command::Send(args) => cmd_send(&args),
        Command::Open(args) => cmd_open(&args),
        Command::Clip(args) => cmd_clip(&args.action),
        Command::Daemon(_) => not_yet("daemon", "M1"),
    }
}

/// `qdrop peers` — reads `peers.toml` today; live status arrives with the
/// control socket in M7.
fn cmd_peers(args: &cli::PeersArgs) -> Result<()> {
    let peers = Peers::load()?;
    if args.json {
        // Minimal hand-rolled JSON keeps M0 free of a serde_json dependency.
        let items: Vec<String> = peers
            .peers
            .iter()
            .map(|p| {
                format!(
                    r#"{{"name":{},"device_id":{},"online":false}}"#,
                    json_str(&p.name),
                    json_str(&p.device_id)
                )
            })
            .collect();
        println!("[{}]", items.join(","));
        return Ok(());
    }

    if peers.is_empty() {
        println!("No paired peers. Run `qdrop pair` to add one.");
        return Ok(());
    }
    println!("{:<20} {:<10} LAST SEEN", "NAME", "STATUS");
    for p in &peers.peers {
        println!(
            "{:<20} {:<10} {}",
            p.name,
            "unknown",
            p.last_seen.as_deref().unwrap_or("never")
        );
    }
    Ok(())
}

fn cmd_pair(args: &cli::PairArgs) -> Result<()> {
    match (&args.remove, &args.target) {
        (Some(name), _) => not_yet(&format!("pair --remove {name}"), "M2"),
        (None, Some(target)) => not_yet(&format!("pair {target}"), "M2"),
        (None, None) => not_yet("pair", "M2"),
    }
}

fn cmd_send(args: &cli::SendArgs) -> Result<()> {
    for path in &args.paths {
        if !path.exists() {
            anyhow::bail!("no such file: {}", path.display());
        }
    }
    not_yet("send", "M4")
}

fn cmd_open(args: &cli::OpenArgs) -> Result<()> {
    let _ = &args.url;
    not_yet("open", "M5")
}

fn cmd_clip(action: &ClipAction) -> Result<()> {
    let name = match action {
        ClipAction::Pause => "clip --pause",
        ClipAction::Resume => "clip --resume",
        ClipAction::Status => "clip --status",
    };
    not_yet(name, "M3")
}

fn not_yet(what: &str, milestone: &str) -> Result<()> {
    anyhow::bail!("`{what}` is not implemented yet (scheduled for {milestone})")
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
