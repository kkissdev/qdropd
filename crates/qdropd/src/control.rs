//! A tiny Unix-socket control channel: newline-delimited JSON requests and
//! responses. `qdrop clip --pause/--resume/--status` drives it. M7 will grow
//! this into the full status/control surface.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::bus::PeerBus;

pub use qdrop_core::control::socket_path;

/// Runtime toggles shared with the sync subsystem.
#[derive(Debug, Default)]
pub struct Controls {
    clipboard_paused: AtomicBool,
}

impl Controls {
    pub fn clipboard_paused(&self) -> bool {
        self.clipboard_paused.load(Ordering::Relaxed)
    }
    pub fn set_clipboard_paused(&self, v: bool) {
        self.clipboard_paused.store(v, Ordering::Relaxed);
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    ClipPause,
    ClipResume,
    ClipStatus,
    Status,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    clipboard_paused: bool,
    connected: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Bind the control socket and serve requests until the task is dropped.
pub fn spawn(controls: Arc<Controls>, bus: PeerBus) -> Result<tokio::task::JoinHandle<()>> {
    let path = socket_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A stale socket file from a previous run would block bind().
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .with_context(|| format!("binding control socket {}", path.display()))?;
    tracing::info!(socket = %path.display(), "control socket ready");

    Ok(tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let controls = controls.clone();
                    let bus = bus.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, controls, bus).await {
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

async fn handle(stream: UnixStream, controls: Arc<Controls>, bus: PeerBus) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }

    let resp = match serde_json::from_str::<Request>(line) {
        Ok(Request::ClipPause) => {
            controls.set_clipboard_paused(true);
            ok_response(&controls, &bus)
        }
        Ok(Request::ClipResume) => {
            controls.set_clipboard_paused(false);
            ok_response(&controls, &bus)
        }
        Ok(Request::ClipStatus) | Ok(Request::Status) => ok_response(&controls, &bus),
        Err(e) => Response {
            ok: false,
            clipboard_paused: controls.clipboard_paused(),
            connected: bus.connected_ids(),
            error: Some(format!("bad request: {e}")),
        },
    };

    let mut out = serde_json::to_vec(&resp)?;
    out.push(b'\n');
    reader.into_inner().write_all(&out).await?;
    Ok(())
}

fn ok_response(controls: &Controls, bus: &PeerBus) -> Response {
    Response {
        ok: true,
        clipboard_paused: controls.clipboard_paused(),
        connected: bus.connected_ids(),
        error: None,
    }
}
