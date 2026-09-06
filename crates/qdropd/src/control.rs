//! A tiny Unix-socket control channel: newline-delimited JSON requests and
//! responses. `qdrop clip`, `qdrop send`, and `qdrop open` drive it. M7 grows
//! this into the full status/control surface.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::auth::AuthManager;
use crate::bus::PeerBus;
use crate::clipboard::ClipboardHandle;
use crate::filexfer::FileXfer;
use crate::history::{History, Restored};

pub use qdrop_core::control::socket_path;

/// Runtime toggles shared with the sync subsystem. `sync_images` mirrors
/// `config.toml` and is refreshed by the config watcher, so the toggle takes
/// effect without a restart.
#[derive(Debug, Default)]
pub struct Controls {
    clipboard_paused: AtomicBool,
    sync_images: AtomicBool,
}

impl Controls {
    pub fn clipboard_paused(&self) -> bool {
        self.clipboard_paused.load(Ordering::Relaxed)
    }
    pub fn set_clipboard_paused(&self, v: bool) {
        self.clipboard_paused.store(v, Ordering::Relaxed);
    }
    pub fn sync_images(&self) -> bool {
        self.sync_images.load(Ordering::Relaxed)
    }
    pub fn set_sync_images(&self, v: bool) {
        self.sync_images.store(v, Ordering::Relaxed);
    }
}

/// Shared handles the control server hands to request handlers.
#[derive(Clone)]
pub struct ControlDeps {
    pub controls: Arc<Controls>,
    pub bus: PeerBus,
    pub filex: Arc<FileXfer>,
    pub auth: Arc<AuthManager>,
    pub clipboard: Option<Arc<ClipboardHandle>>,
    pub history: Arc<History>,
    pub own_id: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    ClipPause,
    ClipResume,
    ClipStatus,
    Status,
    Send {
        paths: Vec<String>,
        #[serde(default)]
        to: Option<String>,
    },
    Open {
        url: String,
        #[serde(default)]
        to: Option<String>,
    },
    Auth {
        name: String,
        #[serde(default)]
        mutual: bool,
    },
    /// `qdrop paste` — the daemon's current clipboard text.
    ClipGet,
    /// `qdrop copy` — set the shared clipboard.
    ClipSet {
        text: String,
    },
    /// `qdrop clip --history`.
    ClipHistory,
    /// `qdrop clip --restore <n>` — put history entry n back on the clipboard.
    ClipRestore {
        index: usize,
    },
    /// `qdrop clip --send <n> [--to name]`.
    ClipSend {
        index: usize,
        #[serde(default)]
        to: Option<String>,
    },
}

/// Bind the control socket and serve requests until the task is dropped.
pub fn spawn(deps: ControlDeps) -> Result<tokio::task::JoinHandle<()>> {
    let path = socket_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&path); // clear a stale socket
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("binding control socket {}", path.display()))?;
    tracing::info!(socket = %path.display(), "control socket ready");

    Ok(tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let deps = deps.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, deps).await {
                            tracing::debug!("control client error: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("control accept error: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }))
}

async fn handle(stream: UnixStream, deps: ControlDeps) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }

    let resp = match serde_json::from_str::<Request>(line) {
        Ok(req) => dispatch(req, &deps).await,
        Err(e) => json!({ "ok": false, "error": format!("bad request: {e}") }),
    };

    let mut out = serde_json::to_vec(&resp)?;
    out.push(b'\n');
    reader.into_inner().write_all(&out).await?;
    Ok(())
}

async fn dispatch(req: Request, deps: &ControlDeps) -> serde_json::Value {
    match req {
        Request::ClipPause => {
            deps.controls.set_clipboard_paused(true);
            status_json(deps)
        }
        Request::ClipResume => {
            deps.controls.set_clipboard_paused(false);
            status_json(deps)
        }
        Request::ClipStatus | Request::Status => status_json(deps),
        Request::Send { paths, to } => {
            let paths: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
            let outcomes = deps.filex.send(paths, to).await;
            let ok = !outcomes.is_empty() && outcomes.iter().all(|o| o.ok);
            json!({ "ok": ok, "sent": outcomes })
        }
        Request::Open { url, to } => {
            match crate::weblink::dispatch(&deps.bus, &url, to.as_deref()) {
                Ok(n) => json!({ "ok": true, "dispatched": n }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        Request::ClipGet => match &deps.clipboard {
            None => json!({ "ok": false, "error": "clipboard unavailable on this host" }),
            Some(h) => {
                let h = h.clone();
                let text = tokio::task::spawn_blocking(move || h.get_text())
                    .await
                    .ok()
                    .flatten();
                json!({ "ok": true, "text": text })
            }
        },
        Request::ClipSet { text } => match &deps.clipboard {
            None => json!({ "ok": false, "error": "clipboard unavailable on this host" }),
            Some(h) => {
                h.apply_text(text);
                json!({ "ok": true })
            }
        },
        Request::ClipHistory => json!({ "ok": true, "entries": deps.history.list().await }),
        Request::ClipRestore { index } => match (deps.history.get(index).await, &deps.clipboard) {
            (Some(Restored::Text(t)), Some(h)) => {
                h.apply_text(t);
                json!({ "ok": true })
            }
            (Some(Restored::Image(b)), Some(h)) => {
                h.apply_image(b);
                json!({ "ok": true })
            }
            (None, _) => json!({ "ok": false, "error": "no such history entry" }),
            (_, None) => json!({ "ok": false, "error": "clipboard unavailable on this host" }),
        },
        Request::ClipSend { index, to } => match deps.history.get(index).await {
            None => json!({ "ok": false, "error": "no such history entry" }),
            Some(Restored::Text(t)) => {
                let msg = qdrop_core::Message::Clipboard {
                    seq: 0,
                    origin_id: deps.own_id.clone(),
                    entries: vec![qdrop_core::proto::ClipEntry::text(&t)],
                };
                let n = match to.as_deref() {
                    Some(name) => {
                        let id = qdrop_core::Peers::load().ok().and_then(|p| {
                            p.find(name)
                                .or_else(|| p.find_by_id(name))
                                .map(|p| p.device_id.clone())
                        });
                        match id {
                            Some(id) if deps.bus.send_to(&id, msg) => 1,
                            _ => 0,
                        }
                    }
                    None => {
                        deps.bus.broadcast(&msg, None);
                        deps.bus.connected_ids().len()
                    }
                };
                json!({ "ok": n > 0, "sent_to": n })
            }
            Some(Restored::Image(b)) => {
                deps.filex.send_clip_image(b).await;
                json!({ "ok": true })
            }
        },
        Request::Auth { name, mutual } => {
            let id = qdrop_core::Peers::load().ok().and_then(|p| {
                p.find(&name)
                    .or_else(|| p.find_by_id(&name))
                    .map(|p| p.device_id.clone())
            });
            match id {
                None => {
                    json!({ "ok": false, "error": format!("no paired peer matching {name:?}") })
                }
                Some(id) => match deps.auth.authorize(&id, mutual).await {
                    Ok(host) => json!({ "ok": true, "hostname": host }),
                    Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
                },
            }
        }
    }
}

fn status_json(deps: &ControlDeps) -> serde_json::Value {
    let connected = deps.bus.connected_ids();
    let peers = qdrop_core::Peers::load().unwrap_or_default();
    let state = qdrop_core::state::DaemonState::load().unwrap_or_default();

    let peer_rows: Vec<serde_json::Value> = peers
        .peers
        .iter()
        .map(|p| {
            let online = connected.contains(&p.device_id);
            json!({
                "name": p.name,
                "device_id": p.device_id,
                "online": online,
                "since_unix": state.online.get(&p.device_id),
                "last_seen_unix": state.last_seen.get(&p.device_id),
            })
        })
        .collect();

    json!({
        "ok": true,
        "version": qdrop_core::VERSION,
        "port": deps.port,
        "clipboard_paused": deps.controls.clipboard_paused(),
        "sync_images": deps.controls.sync_images(),
        "connected": connected,
        "peers": peer_rows,
    })
}
