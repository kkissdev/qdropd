//! `qdrop` — the command-line client.

mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use qdrop_core::pairing::{self, LocalInfo, Paired};
use qdrop_core::state::{ago, DaemonState};
use qdrop_core::{Config, Identity, Peer, Peers};

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
        Command::Pair(args) => cmd_pair(&args, cli.verbose),
        Command::Send(args) => cmd_send(&args),
        Command::Open(args) => cmd_open(&args),
        Command::Clip(args) => cmd_clip(args.action()),
        Command::Status(args) => cmd_status(&args),
        Command::Daemon(args) => cmd_daemon(&args, cli.verbose),
    }
}

/// `qdrop daemon` — thin front-end that runs the `qdropd` binary sitting next
/// to this one. The daemon is a separate executable (it carries the mDNS and
/// transport deps); this keeps one entrypoint for users and packaging (M8).
fn cmd_daemon(args: &cli::DaemonArgs, verbose: bool) -> Result<()> {
    use std::process::Command as Proc;

    let exe = std::env::current_exe().context("locating the qdrop executable")?;
    let qdropd = exe
        .parent()
        .map(|dir| {
            dir.join(if cfg!(windows) {
                "qdropd.exe"
            } else {
                "qdropd"
            })
        })
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("qdropd"));

    let mut cmd = Proc::new(&qdropd);
    if verbose {
        cmd.arg("--verbose");
    }
    if let Some(port) = args.port {
        cmd.arg("--port").arg(port.to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Replace this process so signals and the service manager see qdropd directly.
        let err = cmd.exec();
        Err(anyhow::Error::new(err).context(format!("exec {}", qdropd.display())))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("running {}", qdropd.display()))?;
        std::process::exit(status.code().unwrap_or(1))
    }
}

/// `qdrop peers` — paired peers plus live status from the daemon's
/// `state.json` (online / last seen). A proper control socket arrives in M7.
fn cmd_peers(args: &cli::PeersArgs) -> Result<()> {
    let peers = Peers::load()?;
    let state = DaemonState::load().unwrap_or_default();

    if args.json {
        let rows: Vec<serde_json::Value> = peers
            .peers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "device_id": p.device_id,
                    "public_key": p.public_key,
                    "online": state.is_online(&p.device_id),
                    "last_seen_unix": state
                        .online
                        .get(&p.device_id)
                        .or_else(|| state.last_seen.get(&p.device_id)),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if peers.is_empty() {
        println!("No paired peers. Run `qdrop pair` to add one.");
        return Ok(());
    }
    println!("{:<20} {:<9} LAST SEEN", "NAME", "STATUS");
    for p in &peers.peers {
        let (status, when) = if let Some(since) = state.online.get(&p.device_id) {
            ("online", format!("since {}", ago(*since)))
        } else if let Some(seen) = state.last_seen.get(&p.device_id) {
            ("offline", ago(*seen))
        } else {
            ("offline", "never".to_string())
        };
        println!("{:<20} {:<9} {}", p.name, status, when);
    }
    Ok(())
}

fn cmd_pair(args: &cli::PairArgs, verbose: bool) -> Result<()> {
    if let Some(name) = &args.remove {
        return cmd_unpair(name);
    }

    let config = Config::load()?;
    let identity = Identity::load_or_create().context("loading identity key")?;
    let device_id = qdrop_core::device::load_or_create()?;
    let local = LocalInfo::new(&identity, &device_id, &config.device_name);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;

    let paired = match &args.target {
        None => runtime.block_on(pairing::run_offer(local, |pin| {
            println!("Pairing PIN: {pin}");
            println!(
                "On the other device, run:  qdrop pair {}",
                shell_quote(&config.device_name)
            );
            println!("Waiting up to 60s for it to connect...");
        }))?,
        Some(target) => {
            let pin = prompt_pin(target)?;
            runtime.block_on(pairing::run_request(local, target, pin.trim()))?
        }
    };

    record_peer(&paired)?;
    if verbose {
        tracing::debug!(?paired, "pairing complete");
    }
    println!(
        "Paired with {} ({})",
        paired.device_name,
        qdrop_core::identity::fingerprint_of(&paired.public_key)
    );
    Ok(())
}

fn cmd_unpair(name: &str) -> Result<()> {
    let mut peers = Peers::load()?;
    if peers.remove_by_name(name) {
        peers.save()?;
        println!("Unpaired {name}.");
        Ok(())
    } else {
        anyhow::bail!("no paired peer named {name:?}")
    }
}

fn record_peer(paired: &Paired) -> Result<()> {
    let mut peers = Peers::load()?;
    peers.upsert(Peer::new(
        &paired.device_name,
        &paired.device_id,
        &paired.public_key,
    ));
    peers.save()?;
    Ok(())
}

fn prompt_pin(target: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    print!("Enter the 6-digit PIN shown on {target}: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading PIN from stdin")?;
    let pin = line.trim();
    if pin.len() != 6 || !pin.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("expected a 6-digit PIN, got {pin:?}");
    }
    Ok(line)
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || "-_.".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn cmd_send(args: &cli::SendArgs) -> Result<()> {
    let mut abs = Vec::new();
    for path in &args.paths {
        if !path.is_file() {
            anyhow::bail!("not a file: {}", path.display());
        }
        abs.push(
            std::fs::canonicalize(path)
                .with_context(|| format!("resolving {}", path.display()))?
                .to_string_lossy()
                .into_owned(),
        );
    }

    let req = serde_json::json!({
        "cmd": "send",
        "paths": abs,
        "to": args.to,
    });
    println!(
        "Sending {} file(s){}...",
        abs.len(),
        args.to
            .as_deref()
            .map(|t| format!(" to {t}"))
            .unwrap_or_default()
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let resp = rt.block_on(qdrop_core::control::request_json(&req))?;

    let mut all_ok = true;
    if let Some(sent) = resp.get("sent").and_then(|v| v.as_array()) {
        for s in sent {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let peer = s.get("peer").and_then(|v| v.as_str()).unwrap_or("?");
            let ok = s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            let detail = s.get("detail").and_then(|v| v.as_str()).unwrap_or("");
            if ok {
                println!("  ✓ {name} → {peer}");
            } else {
                all_ok = false;
                println!("  ✗ {name} → {peer}: {detail}");
            }
        }
    } else if let Some(err) = resp.get("error").and_then(|v| v.as_str()) {
        anyhow::bail!("{err}");
    }
    if !all_ok {
        anyhow::bail!("one or more transfers failed");
    }
    Ok(())
}

fn cmd_open(args: &cli::OpenArgs) -> Result<()> {
    if !qdrop_core::proto::is_allowed_url(&args.url) {
        anyhow::bail!(
            "refusing {:?}: only {} URLs are allowed",
            args.url,
            qdrop_core::proto::URL_SCHEME_ALLOWLIST.join(", ")
        );
    }
    let req = serde_json::json!({ "cmd": "open", "url": args.url, "to": args.to });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let resp = rt.block_on(qdrop_core::control::request_json(&req))?;

    if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let n = resp.get("dispatched").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("Opened on {n} peer(s).");
        Ok(())
    } else {
        anyhow::bail!(
            "{}",
            resp.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("failed")
        )
    }
}

fn cmd_status(args: &cli::StatusArgs) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let resp = rt.block_on(qdrop_core::control::request("status"))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    let paused = resp
        .get("clipboard_paused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let empty = vec![];
    let peers = resp
        .get("peers")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let online = peers
        .iter()
        .filter(|p| p.get("online").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();

    if args.waybar {
        let icon = if paused {
            "󰅘"
        } else if online > 0 {
            "󰓅"
        } else {
            "󰤭"
        };
        let tip = peers
            .iter()
            .map(|p| {
                format!(
                    "{}: {}",
                    p.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                    if p.get("online").and_then(|v| v.as_bool()).unwrap_or(false) {
                        "online"
                    } else {
                        "offline"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\\n");
        let class = if paused {
            "paused"
        } else if online > 0 {
            "connected"
        } else {
            "idle"
        };
        println!(
            r#"{{"text":"{icon} {online}","tooltip":"qdrop — clipboard {}\n{tip}","class":"{class}"}}"#,
            if paused { "paused" } else { "active" }
        );
        return Ok(());
    }

    println!(
        "qdrop {} — clipboard sync {}",
        resp.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
        if paused { "PAUSED" } else { "active" }
    );
    if peers.is_empty() {
        println!("No paired peers.");
    } else {
        println!("{:<20} STATUS", "PEER");
        for p in peers {
            println!(
                "{:<20} {}",
                p.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                if p.get("online").and_then(|v| v.as_bool()).unwrap_or(false) {
                    "online"
                } else {
                    "offline"
                }
            );
        }
    }
    Ok(())
}

fn cmd_clip(action: ClipAction) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;

    let cmd = match action {
        ClipAction::Pause => "clip_pause",
        ClipAction::Resume => "clip_resume",
        ClipAction::Status => "clip_status",
        ClipAction::Toggle => {
            let now = rt.block_on(qdrop_core::control::request("clip_status"))?;
            if now
                .get("clipboard_paused")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "clip_resume"
            } else {
                "clip_pause"
            }
        }
    };
    let resp = rt.block_on(qdrop_core::control::request(cmd))?;

    let paused = resp
        .get("clipboard_paused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let connected = resp
        .get("connected")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    match action {
        ClipAction::Status => {
            println!(
                "Clipboard sync: {}",
                if paused { "paused" } else { "active" }
            );
            println!("Connected peers: {connected}");
        }
        _ => println!(
            "Clipboard sync {}.",
            if paused { "paused" } else { "resumed" }
        ),
    }
    Ok(())
}
