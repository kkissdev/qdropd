//! The peer bus: a shared map of the currently-connected peers' outbound
//! message channels, so subsystems (clipboard sync, file send, …) can push a
//! frame to one peer or all of them without knowing about connections.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use qdrop_core::proto::Message;
use tokio::sync::mpsc;

/// device id -> that connection's outbound sender.
#[derive(Clone, Default)]
pub struct PeerBus(Arc<Mutex<HashMap<String, mpsc::Sender<Message>>>>);

impl PeerBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, device_id: String, tx: mpsc::Sender<Message>) {
        self.0.lock().unwrap().insert(device_id, tx);
    }

    pub fn remove(&self, device_id: &str) {
        self.0.lock().unwrap().remove(device_id);
    }

    pub fn connected_ids(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }

    #[allow(dead_code)] // used from M4 (file send) onward
    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }

    /// Send `msg` to every connected peer except `except`. Uses `try_send`, so
    /// a slow peer drops the update rather than stalling everyone.
    pub fn broadcast(&self, msg: &Message, except: Option<&str>) {
        let targets: Vec<mpsc::Sender<Message>> = {
            let guard = self.0.lock().unwrap();
            guard
                .iter()
                .filter(|(id, _)| except != Some(id.as_str()))
                .map(|(_, tx)| tx.clone())
                .collect()
        };
        for tx in targets {
            let _ = tx.try_send(msg.clone());
        }
    }

    /// Send `msg` to a single peer. Returns whether it was queued.
    #[allow(dead_code)] // used from M4 (file send) onward
    pub fn send_to(&self, device_id: &str, msg: Message) -> bool {
        let tx = self.0.lock().unwrap().get(device_id).cloned();
        matches!(tx, Some(tx) if tx.try_send(msg).is_ok())
    }
}
