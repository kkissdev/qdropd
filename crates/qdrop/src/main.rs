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
        Command::Send(args) => cmd_send(&args, cli.quiet),
        Command::Recv(args) => cmd_recv(&args, cli.quiet),
        Command::Paste => cmd_paste(),
        Command::Copy => cmd_copy(cli.quiet),
        Command::Open(args) => cmd_open(&args),
        Command::Clip(args) => cmd_clip(args.action()),
        Command::Auth(args) => cmd_auth(&args),
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

fn cmd_send(args: &cli::SendArgs, quiet: bool) -> Result<()> {
    // `qdrop send -` reads a single file from stdin into a temp file whose
    // basename is what the peer will see.
    let _stdin_temp;
    let mut abs = Vec::new();
    if args.paths.len() == 1 && args.paths[0].as_os_str() == "-" {
        use std::io::Read;
        let name = args.name.clone().unwrap_or_else(|| {
            format!(
                "stdin-{}.bin",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            )
        });
        let name =
            qdrop_core::proto::safe_blob_name(&name).context("--name is not a valid filename")?;
        let dir = std::env::temp_dir().join(format!("qdrop-send-{}", std::process::id()));
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(&name);
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("reading stdin")?;
        std::fs::write(&path, &buf).with_context(|| format!("writing {}", path.display()))?;
        abs.push(path.to_string_lossy().into_owned());
        _stdin_temp = TempDir(dir);
    } else {
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
    }

    let req = serde_json::json!({ "cmd": "send", "paths": abs, "to": args.to });
    if !quiet {
        println!(
            "Sending {} file(s){}...",
            abs.len(),
            args.to
                .as_deref()
                .map(|t| format!(" to {t}"))
                .unwrap_or_default()
        );
    }

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
                if !quiet {
                    println!("  ✓ {name} → {peer}");
                }
            } else {
                all_ok = false;
                eprintln!("  ✗ {name} → {peer}: {detail}");
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

/// Removes its directory tree on drop.
struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cmd_recv(args: &cli::RecvArgs, quiet: bool) -> Result<()> {
    use std::io::Write;
    let dir = qdrop_core::paths::downloads_dir()?;
    std::fs::create_dir_all(&dir).ok();

    let snapshot = |d: &std::path::Path| -> std::collections::HashSet<std::path::PathBuf> {
        std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .starts_with('.')
            })
            .collect()
    };
    let before = snapshot(&dir);
    if !quiet {
        eprintln!("Waiting for an incoming file in {}...", dir.display());
    }

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(args.timeout.unwrap_or(300));
    let new_file = loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for a file");
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        let now = snapshot(&dir);
        if let Some(p) = now.difference(&before).next().cloned() {
            // wait for the size to settle (transfer finished + renamed)
            let s1 = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let s2 = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(1);
            if s1 == s2 {
                break p;
            }
        }
    };

    if args.stdout {
        let bytes = std::fs::read(&new_file)?;
        std::io::stdout().write_all(&bytes)?;
        if !args.keep {
            let _ = std::fs::remove_file(&new_file);
        }
    } else {
        println!("{}", new_file.display());
    }
    Ok(())
}

fn cmd_paste() -> Result<()> {
    use std::io::Write;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let resp = rt.block_on(qdrop_core::control::request("clip_get"))?;
    if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        anyhow::bail!(
            "{}",
            resp.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("failed")
        );
    }
    if let Some(t) = resp.get("text").and_then(|v| v.as_str()) {
        std::io::stdout().write_all(t.as_bytes())?;
    }
    Ok(())
}

fn cmd_copy(quiet: bool) -> Result<()> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .context("reading stdin")?;
    let req = serde_json::json!({ "cmd": "clip_set", "text": text });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let resp = rt.block_on(qdrop_core::control::request_json(&req))?;
    if !resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        anyhow::bail!(
            "{}",
            resp.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("failed")
        );
    }
    if !quiet {
        eprintln!("Copied {} bytes to the shared clipboard.", text.len());
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

fn cmd_auth(args: &cli::AuthArgs) -> Result<()> {
    // Revoke — edit peers.toml directly, no daemon needed.
    if let Some(name) = &args.remove {
        let mut peers = Peers::load()?;
        match peers.find_any_mut(name) {
            Some(p) if p.authorized => {
                p.authorized = false;
                peers.save()?;
                println!("Revoked authorization for {name}.");
            }
            Some(_) => println!("{name} was not authorized."),
            None => anyhow::bail!("no paired peer matching {name:?}"),
        }
        return Ok(());
    }

    // No argument — list authorized peers.
    let Some(name) = &args.name else {
        let peers = Peers::load()?;
        let authed: Vec<_> = peers.peers.iter().filter(|p| p.authorized).collect();
        if authed.is_empty() {
            println!("No authorized peers. Run `qdrop auth <name>` while connected to one.");
            return Ok(());
        }
        println!("{:<20} {:<24} MACs", "PEER", "HOSTNAME");
        for p in authed {
            println!(
                "{:<20} {:<24} {}",
                p.name,
                p.hostname.as_deref().unwrap_or("-"),
                if p.macs.is_empty() {
                    "-".into()
                } else {
                    p.macs.join(", ")
                }
            );
        }
        return Ok(());
    };

    let req = serde_json::json!({ "cmd": "auth", "name": name, "mutual": args.mutual });
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let resp = rt.block_on(qdrop_core::control::request_json(&req))?;

    if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let host = resp.get("hostname").and_then(|v| v.as_str()).unwrap_or("?");
        println!("Authorized {name} ({host}) — it can now send here unattended.");
        if args.mutual {
            println!("Asked {name} to authorize this machine back.");
        }
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
