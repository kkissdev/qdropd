//! Clipboard history (M14): a small ring of recent clipboard entries, text and
//! images, backed by `~/.config/qdrop/history/`. Concealed / transient items
//! never reach here (the clipboard thread drops them first).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use qdrop_core::crypto::sha256;
use qdrop_core::state::now_unix;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Body {
    Text { text: String },
    Image { file: String, w: u32, h: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(flatten)]
    body: Body,
    when: u64,
    origin: String,
    /// Hex SHA-256 of the content, for dedup.
    #[serde(default)]
    hash: String,
}

impl Entry {
    fn preview(&self) -> String {
        match &self.body {
            Body::Text { text } => {
                let t: String = text.chars().take(60).collect();
                t.replace('\n', "⏎")
            }
            Body::Image { w, h, .. } => format!("[image {w}×{h}]"),
        }
    }
}

/// Restored content, ready to put back on the clipboard or send.
pub enum Restored {
    Text(String),
    Image(Vec<u8>),
}

pub struct History {
    dir: PathBuf,
    max: usize,
    entries: Mutex<VecDeque<Entry>>,
    next_file: std::sync::atomic::AtomicU64,
}

impl History {
    /// Load the on-disk history (or an empty one). `max == 0` disables it.
    pub fn load(dir: PathBuf, max: usize) -> Arc<Self> {
        let mut entries: VecDeque<Entry> = std::fs::read(dir.join("index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        entries.truncate(max);
        let next = entries
            .iter()
            .filter_map(|e| match &e.body {
                Body::Image { file, .. } => file
                    .strip_suffix(".png")
                    .and_then(|s| s.parse::<u64>().ok()),
                _ => None,
            })
            .max()
            .map(|n| n + 1)
            .unwrap_or(0);
        Arc::new(Self {
            dir,
            max,
            entries: Mutex::new(entries),
            next_file: std::sync::atomic::AtomicU64::new(next),
        })
    }

    pub fn enabled(&self) -> bool {
        self.max > 0
    }

    pub async fn record_text(&self, text: &str, origin: &str) {
        if !self.enabled() || text.is_empty() {
            return;
        }
        self.push(Entry {
            hash: hex(&sha256(text.as_bytes())),
            body: Body::Text {
                text: text.to_string(),
            },
            when: now_unix(),
            origin: origin.to_string(),
        })
        .await;
    }

    pub async fn record_image(&self, png: &[u8], origin: &str) {
        if !self.enabled() {
            return;
        }
        let content_hash = hex(&sha256(png));
        // Cheap dedup before writing a file.
        if self
            .entries
            .lock()
            .await
            .front()
            .is_some_and(|f| f.hash == content_hash)
        {
            return;
        }
        let (w, h) = image::load_from_memory(png)
            .map(|i| (i.width(), i.height()))
            .unwrap_or((0, 0));
        let n = self
            .next_file
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = format!("{n}.png");
        if std::fs::create_dir_all(&self.dir)
            .and_then(|_| std::fs::write(self.dir.join(&file), png))
            .is_err()
        {
            return;
        }
        self.push(Entry {
            hash: content_hash,
            body: Body::Image { file, w, h },
            when: now_unix(),
            origin: origin.to_string(),
        })
        .await;
    }

    async fn push(&self, entry: Entry) {
        let mut q = self.entries.lock().await;
        if q.front().is_some_and(|f| f.hash == entry.hash) {
            return; // exact repeat of the newest entry
        }
        q.push_front(entry);
        while q.len() > self.max {
            if let Some(old) = q.pop_back() {
                if let Body::Image { file, .. } = old.body {
                    let _ = std::fs::remove_file(self.dir.join(file));
                }
            }
        }
        self.persist(&q);
    }

    fn persist(&self, q: &VecDeque<Entry>) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(q) {
            let _ = qdrop_core::paths::write_atomic(&self.dir.join("index.json"), &bytes);
        }
    }

    /// Entries newest-first, as JSON for the control socket.
    pub async fn list(&self) -> Vec<serde_json::Value> {
        let peers = qdrop_core::Peers::load().unwrap_or_default();
        let name_of = |origin: &str| -> String {
            if origin == "local" {
                "local".into()
            } else {
                peers
                    .find_by_id(origin)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| origin.chars().take(8).collect())
            }
        };
        self.entries
            .lock()
            .await
            .iter()
            .enumerate()
            .map(|(i, e)| {
                json!({
                    "index": i,
                    "kind": match e.body { Body::Text { .. } => "text", Body::Image { .. } => "image" },
                    "preview": e.preview(),
                    "when_unix": e.when,
                    "origin": name_of(&e.origin),
                })
            })
            .collect()
    }

    /// Fetch entry `index` for restore / send.
    pub async fn get(&self, index: usize) -> Option<Restored> {
        let q = self.entries.lock().await;
        let e = q.get(index)?;
        match &e.body {
            Body::Text { text } => Some(Restored::Text(text.clone())),
            Body::Image { file, .. } => {
                std::fs::read(self.dir.join(file)).ok().map(Restored::Image)
            }
        }
    }
}

/// Default history directory.
pub fn dir() -> PathBuf {
    qdrop_core::paths::config_dir()
        .map(|d| d.join("history"))
        .unwrap_or_else(|_| PathBuf::from("qdrop-history"))
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ring_evicts_and_dedups() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::load(dir.path().to_path_buf(), 3);
        h.record_text("a", "me").await;
        h.record_text("a", "me").await; // dup -> ignored
        h.record_text("b", "peer").await;
        h.record_text("c", "me").await;
        h.record_text("d", "me").await; // evicts "a"

        let list = h.list().await;
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["preview"], "d");
        assert_eq!(list[2]["preview"], "b");
        assert_eq!(list[2]["origin"], "peer");

        match h.get(1).await.unwrap() {
            Restored::Text(t) => assert_eq!(t, "c"),
            _ => panic!("expected text"),
        }

        // Reload from disk keeps the ring.
        let h2 = History::load(dir.path().to_path_buf(), 3);
        assert_eq!(h2.list().await.len(), 3);
    }

    #[tokio::test]
    async fn disabled_when_max_zero() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::load(dir.path().to_path_buf(), 0);
        h.record_text("x", "me").await;
        assert!(h.list().await.is_empty());
    }
}
