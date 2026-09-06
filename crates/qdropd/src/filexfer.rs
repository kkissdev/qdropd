//! File transfer: `qdrop send` streams a file to a peer as
//! `BlobStart` / `BlobChunk` / `BlobEnd`, the peer verifies the SHA-256 and
//! atomically renames it into `~/Downloads/qdrop/`, and acks the result.
//!
//! One blob is in flight per peer at a time (the sender serialises), and the
//! bounded outbound channel provides backpressure. A drop mid-transfer aborts
//! that blob; the send is retried from the start when the peer reconnects.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use qdrop_core::proto::{safe_blob_name, BlobPurpose, Message, BLOB_CHUNK};
use qdrop_core::Config;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;

use crate::bus::PeerBus;
use crate::notify::notify;

const ACK_TIMEOUT: Duration = Duration::from_secs(120);
const RECONNECT_WAIT: Duration = Duration::from_secs(30);
const SEND_ATTEMPTS: usize = 3;

/// Per-send result reported back to the CLI.
#[derive(Debug, serde::Serialize)]
pub struct SendOutcome {
    pub name: String,
    pub peer: String,
    pub ok: bool,
    pub detail: String,
}

/// Cap on an in-memory (clipboard-image) blob.
const MAX_MEM_BLOB: u64 = 64 * 1024 * 1024;
/// Cap on concurrent incoming transfers (across all peers).
const MAX_CONCURRENT_INCOMING: usize = 16;

pub struct FileXfer {
    bus: PeerBus,
    download_dir: PathBuf,
    require_confirm: bool,
    next_id: AtomicU64,
    /// transfer id -> ack waiter (sender side).
    acks: Mutex<HashMap<u64, oneshot::Sender<(bool, String)>>>,
    /// (peer_id, transfer id) -> in-progress incoming blob (receiver side).
    incoming: Mutex<HashMap<(String, u64), Incoming>>,
    /// Where completed clipboard-image blobs go (set by the sync subsystem).
    clip_sink: Option<mpsc::Sender<Vec<u8>>>,
}

enum Sink {
    File {
        tmp_path: PathBuf,
        final_dir: PathBuf,
        file: fs::File,
    },
    Memory(Vec<u8>),
}

struct Incoming {
    name: String,
    sink: Sink,
    hasher: Sha256,
    written: u64,
    declared: u64,
    peer: String,
    started: std::time::Instant,
}

impl FileXfer {
    pub fn new(
        config: &Config,
        bus: PeerBus,
        download_dir: PathBuf,
        clip_sink: Option<mpsc::Sender<Vec<u8>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            bus,
            download_dir,
            require_confirm: config.require_confirm,
            next_id: AtomicU64::new(1),
            acks: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            clip_sink,
        })
    }

    /// Route an inbound blob-related frame here (from the frame router).
    pub async fn handle_inbound(self: &Arc<Self>, peer_id: String, msg: Message) {
        match msg {
            Message::BlobStart {
                id,
                name,
                size,
                purpose,
            } => self.on_start(peer_id, id, name, size, purpose).await,
            Message::BlobChunk { id, data } => self.on_chunk(peer_id, id, data).await,
            Message::BlobEnd { id, sha256 } => self.on_end(peer_id, id, sha256).await,
            Message::BlobAck { id, ok, detail } => {
                if let Some(tx) = self.acks.lock().await.remove(&id) {
                    let _ = tx.send((ok, detail));
                }
            }
            _ => {}
        }
    }

    async fn reject(&self, peer_id: &str, id: u64, detail: &str) {
        self.bus.send_to(
            peer_id,
            Message::BlobAck {
                id,
                ok: false,
                detail: detail.to_string(),
            },
        );
        tracing::warn!(peer = %peer_id, id, "rejected incoming blob: {detail}");
    }

    async fn on_start(
        self: &Arc<Self>,
        peer_id: String,
        id: u64,
        name: String,
        size: u64,
        purpose: BlobPurpose,
    ) {
        if self.incoming.lock().await.len() >= MAX_CONCURRENT_INCOMING {
            self.reject(&peer_id, id, "too many concurrent transfers")
                .await;
            return;
        }
        let (name, sink) = match purpose {
            BlobPurpose::ClipboardImage => {
                if self.clip_sink.is_none() {
                    self.reject(&peer_id, id, "clipboard image sync not available")
                        .await;
                    return;
                }
                if size > MAX_MEM_BLOB {
                    self.reject(&peer_id, id, "clipboard image too large").await;
                    return;
                }
                ("clipboard-image".to_string(), Sink::Memory(Vec::new()))
            }
            BlobPurpose::File => {
                let Some(safe) = safe_blob_name(&name) else {
                    self.reject(&peer_id, id, "unsafe filename").await;
                    return;
                };
                let final_dir = if self.require_confirm {
                    self.download_dir.join("pending")
                } else {
                    self.download_dir.clone()
                };
                if let Err(e) = fs::create_dir_all(&final_dir).await {
                    self.reject(&peer_id, id, &format!("cannot create download dir: {e}"))
                        .await;
                    return;
                }
                let tmp_path = final_dir.join(format!(".{safe}.{id}.part"));
                let file = match fs::File::create(&tmp_path).await {
                    Ok(f) => f,
                    Err(e) => {
                        self.reject(&peer_id, id, &format!("cannot create temp file: {e}"))
                            .await;
                        return;
                    }
                };
                tracing::info!(peer = %peer_id, id, name = %safe, size, "receiving file");
                (
                    safe,
                    Sink::File {
                        tmp_path,
                        final_dir,
                        file,
                    },
                )
            }
        };
        self.incoming.lock().await.insert(
            (peer_id.clone(), id),
            Incoming {
                name,
                sink,
                hasher: Sha256::new(),
                written: 0,
                declared: size,
                peer: peer_id,
                started: std::time::Instant::now(),
            },
        );
    }

    async fn on_chunk(self: &Arc<Self>, peer_id: String, id: u64, data: Vec<u8>) {
        let mut map = self.incoming.lock().await;
        let Some(inc) = map.get_mut(&(peer_id.clone(), id)) else {
            return;
        };
        match &mut inc.sink {
            Sink::File { file, .. } => {
                if let Err(e) = file.write_all(&data).await {
                    tracing::warn!(peer = %peer_id, id, "write failed: {e}");
                    if let Some(inc) = map.remove(&(peer_id.clone(), id)) {
                        if let Sink::File { tmp_path, .. } = inc.sink {
                            let _ = fs::remove_file(&tmp_path).await;
                        }
                    }
                    drop(map);
                    self.reject(&peer_id, id, "write failed").await;
                    return;
                }
            }
            Sink::Memory(buf) => {
                if buf.len() as u64 + data.len() as u64 > MAX_MEM_BLOB {
                    map.remove(&(peer_id.clone(), id));
                    drop(map);
                    self.reject(&peer_id, id, "clipboard image too large").await;
                    return;
                }
                buf.extend_from_slice(&data);
            }
        }
        inc.hasher.update(&data);
        inc.written += data.len() as u64;
    }

    async fn on_end(self: &Arc<Self>, peer_id: String, id: u64, sha: [u8; 32]) {
        let Some(mut inc) = self.incoming.lock().await.remove(&(peer_id.clone(), id)) else {
            return;
        };

        async fn cleanup(sink: &Sink) {
            if let Sink::File { tmp_path, .. } = sink {
                let _ = fs::remove_file(tmp_path).await;
            }
        }

        if let Sink::File { file, .. } = &mut inc.sink {
            let _ = file.flush().await;
            let _ = file.sync_all().await;
        }

        let got: [u8; 32] = inc.hasher.clone().finalize().into();
        if got != sha {
            cleanup(&inc.sink).await;
            self.reject(&peer_id, id, "sha256 mismatch").await;
            return;
        }
        if inc.declared != 0 && inc.written != inc.declared {
            cleanup(&inc.sink).await;
            self.reject(&peer_id, id, "size mismatch").await;
            return;
        }

        match inc.sink {
            Sink::Memory(bytes) => {
                if let Some(tx) = &self.clip_sink {
                    let _ = tx.send(bytes).await;
                }
                tracing::info!(peer = %inc.peer, bytes = inc.written, "clipboard image received");
                self.bus.send_to(
                    &peer_id,
                    Message::BlobAck {
                        id,
                        ok: true,
                        detail: "clipboard-image".into(),
                    },
                );
            }
            Sink::File {
                tmp_path,
                final_dir,
                file,
            } => {
                drop(file);
                let dest = unique_dest(&final_dir, &inc.name).await;
                if let Err(e) = fs::rename(&tmp_path, &dest).await {
                    let _ = fs::remove_file(&tmp_path).await;
                    self.reject(&peer_id, id, &format!("rename failed: {e}"))
                        .await;
                    return;
                }
                let secs = inc.started.elapsed().as_secs_f64().max(0.001);
                let mbps = (inc.written as f64 / 1.0e6) / secs;
                tracing::info!(
                    peer = %inc.peer,
                    path = %dest.display(),
                    bytes = inc.written,
                    mb_per_s = format_args!("{mbps:.1}"),
                    "file received"
                );
                self.bus.send_to(
                    &peer_id,
                    Message::BlobAck {
                        id,
                        ok: true,
                        detail: inc.name.clone(),
                    },
                );
                let where_ = if self.require_confirm {
                    format!("{} (pending review)", dest.display())
                } else {
                    dest.display().to_string()
                };
                notify(
                    &format!("Received {}", inc.name),
                    &format!("From a peer → {where_}"),
                );
            }
        }
    }

    /// Stream `paths` to `to` (a peer name) or to every connected peer.
    pub async fn send(
        self: &Arc<Self>,
        paths: Vec<PathBuf>,
        to: Option<String>,
    ) -> Vec<SendOutcome> {
        let peers: Vec<String> = match &to {
            Some(name) if self.bus.sender(name).is_some() => vec![name.clone()],
            Some(name) => match resolve_peer(name) {
                Some(id) => vec![id],
                None => {
                    return vec![SendOutcome {
                        name: String::new(),
                        peer: name.clone(),
                        ok: false,
                        detail: "no such paired peer".into(),
                    }]
                }
            },
            None => self.bus.connected_ids(),
        };
        if peers.is_empty() {
            return vec![SendOutcome {
                name: String::new(),
                peer: to.unwrap_or_default(),
                ok: false,
                detail: "no connected peers".into(),
            }];
        }

        let mut tasks = Vec::new();
        for peer in peers {
            let me = self.clone();
            let paths = paths.clone();
            tasks.push(tokio::spawn(async move {
                let mut out = Vec::new();
                for path in paths {
                    out.push(me.send_one_with_retry(&peer, &path).await);
                }
                out
            }));
        }
        let mut outcomes = Vec::new();
        for t in tasks {
            if let Ok(mut v) = t.await {
                outcomes.append(&mut v);
            }
        }
        outcomes
    }

    async fn send_one_with_retry(
        self: &Arc<Self>,
        peer: &str,
        path: &std::path::Path,
    ) -> SendOutcome {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let mut last = String::new();
        for attempt in 1..=SEND_ATTEMPTS {
            match self.send_one(peer, path).await {
                Ok(detail) => {
                    return SendOutcome {
                        name,
                        peer: peer_label(peer),
                        ok: true,
                        detail,
                    }
                }
                Err(e) => {
                    last = format!("{e:#}");
                    tracing::warn!(peer, file = %name, attempt, "send failed: {last}");
                    // Wait for the peer to come back before retrying.
                    let deadline = tokio::time::Instant::now() + RECONNECT_WAIT;
                    while self.bus.sender(peer).is_none() {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    if self.bus.sender(peer).is_none() {
                        break;
                    }
                }
            }
        }
        SendOutcome {
            name,
            peer: peer_label(peer),
            ok: false,
            detail: last,
        }
    }

    async fn send_one(self: &Arc<Self>, peer: &str, path: &std::path::Path) -> Result<String> {
        let sender = self.bus.sender(peer).context("peer is not connected")?;
        let meta = fs::metadata(path)
            .await
            .with_context(|| format!("stat {}", path.display()))?;
        anyhow::ensure!(meta.is_file(), "{} is not a regular file", path.display());
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("path has no filename")?
            .to_string();
        let size = meta.len();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let (ack_tx, ack_rx) = oneshot::channel();
        self.acks.lock().await.insert(id, ack_tx);
        let result = stream_blob(
            &sender,
            id,
            &name,
            size,
            BlobPurpose::File,
            Source::File(path.to_path_buf()),
            ack_rx,
        )
        .await;
        self.acks.lock().await.remove(&id);
        result
    }

    /// Broadcast a clipboard image to every connected peer via the blob path
    /// (used for images larger than the inline limit). Best-effort.
    pub async fn send_clip_image(self: &Arc<Self>, png: Vec<u8>) {
        let size = png.len() as u64;
        for peer in self.bus.connected_ids() {
            let Some(sender) = self.bus.sender(&peer) else {
                continue;
            };
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (ack_tx, ack_rx) = oneshot::channel();
            self.acks.lock().await.insert(id, ack_tx);
            let r = stream_blob(
                &sender,
                id,
                "clipboard.png",
                size,
                BlobPurpose::ClipboardImage,
                Source::Mem(png.clone()),
                ack_rx,
            )
            .await;
            self.acks.lock().await.remove(&id);
            if let Err(e) = r {
                tracing::debug!(peer = %peer, "clipboard image send failed: {e:#}");
            }
        }
    }
}

enum Source {
    File(PathBuf),
    Mem(Vec<u8>),
}

#[allow(clippy::too_many_arguments)]
async fn stream_blob(
    sender: &mpsc::Sender<Message>,
    id: u64,
    name: &str,
    size: u64,
    purpose: BlobPurpose,
    source: Source,
    ack_rx: oneshot::Receiver<(bool, String)>,
) -> Result<String> {
    sender
        .send(Message::BlobStart {
            id,
            name: name.to_string(),
            size,
            purpose,
        })
        .await
        .map_err(|_| anyhow::anyhow!("connection closed"))?;

    let mut hasher = Sha256::new();
    let send_chunk = |data: Vec<u8>| async {
        sender
            .send(Message::BlobChunk { id, data })
            .await
            .map_err(|_| anyhow::anyhow!("connection closed mid-transfer"))
    };
    match source {
        Source::File(p) => {
            let mut file = fs::File::open(&p).await?;
            let mut buf = vec![0u8; BLOB_CHUNK];
            loop {
                let n = file.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                send_chunk(buf[..n].to_vec()).await?;
            }
        }
        Source::Mem(b) => {
            for chunk in b.chunks(BLOB_CHUNK) {
                hasher.update(chunk);
                send_chunk(chunk.to_vec()).await?;
            }
        }
    }
    sender
        .send(Message::BlobEnd {
            id,
            sha256: hasher.finalize().into(),
        })
        .await
        .map_err(|_| anyhow::anyhow!("connection closed"))?;

    match timeout(ACK_TIMEOUT, ack_rx).await {
        Ok(Ok((true, detail))) => Ok(detail),
        Ok(Ok((false, detail))) => Err(anyhow::anyhow!("peer rejected: {detail}")),
        Ok(Err(_)) => Err(anyhow::anyhow!("ack channel dropped")),
        Err(_) => Err(anyhow::anyhow!("timed out waiting for peer ack")),
    }
}

fn resolve_peer(name: &str) -> Option<String> {
    let peers = qdrop_core::Peers::load().ok()?;
    peers
        .find(name)
        .or_else(|| peers.find_by_id(name))
        .map(|p| p.device_id.clone())
}

/// Friendly name for a device id, falling back to the id itself.
pub(crate) fn peer_label(device_id: &str) -> String {
    qdrop_core::Peers::load()
        .ok()
        .and_then(|p| p.find_by_id(device_id).map(|p| p.name.clone()))
        .unwrap_or_else(|| device_id.to_string())
}

async fn unique_dest(dir: &std::path::Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::metadata(&first).await.is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    for i in 1..10_000 {
        let cand = dir.join(format!("{stem} ({i}){ext}"));
        if fs::metadata(&cand).await.is_err() {
            return cand;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdrop_core::proto::Message;

    fn cfg(require_confirm: bool) -> Config {
        Config {
            require_confirm,
            ..Config::default()
        }
    }

    /// Cross-connect two FileXfer instances over in-memory channels.
    fn pair(dir_a: PathBuf, dir_b: PathBuf, confirm_b: bool) -> (Arc<FileXfer>, Arc<FileXfer>) {
        let (a2b_tx, mut a2b_rx) = mpsc::channel::<Message>(256);
        let (b2a_tx, mut b2a_rx) = mpsc::channel::<Message>(256);

        let bus_a = PeerBus::new();
        bus_a.insert("B".into(), a2b_tx);
        let bus_b = PeerBus::new();
        bus_b.insert("A".into(), b2a_tx);

        let fx_a = FileXfer::new(&cfg(false), bus_a, dir_a, None);
        let fx_b = FileXfer::new(&cfg(confirm_b), bus_b, dir_b, None);

        {
            let fx_b = fx_b.clone();
            tokio::spawn(async move {
                while let Some(m) = a2b_rx.recv().await {
                    fx_b.handle_inbound("A".into(), m).await;
                }
            });
        }
        {
            let fx_a = fx_a.clone();
            tokio::spawn(async move {
                while let Some(m) = b2a_rx.recv().await {
                    fx_a.handle_inbound("B".into(), m).await;
                }
            });
        }
        (fx_a, fx_b)
    }

    #[tokio::test]
    async fn round_trips_multiple_files_and_verifies() {
        let src = tempfile::tempdir().unwrap();
        let dl_a = tempfile::tempdir().unwrap();
        let dl_b = tempfile::tempdir().unwrap();

        let f1 = src.path().join("one.txt");
        let f2 = src.path().join("two.bin");
        std::fs::write(&f1, "hello world").unwrap();
        std::fs::write(&f2, vec![0xABu8; 300_000]).unwrap(); // spans several chunks

        let (fx_a, _fx_b) = pair(dl_a.path().into(), dl_b.path().into(), false);
        let out = fx_a.send(vec![f1, f2], Some("B".into())).await;

        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|o| o.ok), "{out:?}");
        assert_eq!(
            std::fs::read_to_string(dl_b.path().join("one.txt")).unwrap(),
            "hello world"
        );
        assert_eq!(
            std::fs::read(dl_b.path().join("two.bin")).unwrap().len(),
            300_000
        );
    }

    #[tokio::test]
    async fn corrupted_transfer_is_rejected_and_leaves_no_file() {
        let dl_b = tempfile::tempdir().unwrap();
        let (_fx_a, fx_b) = pair(
            tempfile::tempdir().unwrap().path().into(),
            dl_b.path().into(),
            false,
        );

        fx_b.handle_inbound(
            "A".into(),
            Message::BlobStart {
                id: 7,
                name: "evil.txt".into(),
                size: 3,
                purpose: qdrop_core::proto::BlobPurpose::File,
            },
        )
        .await;
        fx_b.handle_inbound(
            "A".into(),
            Message::BlobChunk {
                id: 7,
                data: vec![1, 2, 3],
            },
        )
        .await;
        fx_b.handle_inbound(
            "A".into(),
            Message::BlobEnd {
                id: 7,
                sha256: [0u8; 32],
            },
        )
        .await;

        assert!(!dl_b.path().join("evil.txt").exists());
        // no stray temp files
        let leftover: Vec<_> = std::fs::read_dir(dl_b.path()).unwrap().collect();
        assert!(leftover.is_empty(), "temp file left behind");
    }

    #[tokio::test]
    async fn path_traversal_name_is_refused() {
        let dl_b = tempfile::tempdir().unwrap();
        let (_a, fx_b) = pair(
            tempfile::tempdir().unwrap().path().into(),
            dl_b.path().into(),
            false,
        );
        fx_b.handle_inbound(
            "A".into(),
            Message::BlobStart {
                id: 1,
                name: "../../etc/x".into(),
                size: 1,
                purpose: qdrop_core::proto::BlobPurpose::File,
            },
        )
        .await;
        assert!(fx_b.incoming.lock().await.is_empty());
    }

    #[tokio::test]
    async fn require_confirm_routes_to_pending() {
        let src = tempfile::tempdir().unwrap();
        let dl_b = tempfile::tempdir().unwrap();
        let f = src.path().join("doc.txt");
        std::fs::write(&f, "x").unwrap();

        let (fx_a, _b) = pair(
            tempfile::tempdir().unwrap().path().into(),
            dl_b.path().into(),
            true,
        );
        let out = fx_a.send(vec![f], Some("B".into())).await;
        assert!(out[0].ok);
        assert!(dl_b.path().join("pending").join("doc.txt").exists());
        assert!(!dl_b.path().join("doc.txt").exists());
    }
}
