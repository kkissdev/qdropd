//! Connection manager: turns discovery events into exactly one live
//! connection per peer.
//!
//! Arbitration: the device with the lexicographically **lower** id dials; the
//! other only accepts. Since just one side ever dials, there is naturally one
//! connection per pair. The dialer owns a reconnect loop with exponential
//! backoff; the acceptor waits for inbound and replaces a stale connection
//! when a fresh one arrives.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use qdrop_core::proto::{Caps, Hello};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::backoff::Backoff;
use crate::connection::{self, handshake, DisconnectReason, KeepAlive};
use crate::discovery::DiscoveryEvent;

/// Observability hook — one event per connection state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    PeerConnected {
        device_id: String,
        device_name: String,
        caps: Caps,
    },
    PeerDisconnected {
        device_id: String,
        reason: String,
    },
}

/// Tuning for the manager and its connections.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Our own `Hello` (identity + advertised caps).
    pub local: Hello,
    pub keepalive: KeepAlive,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub connect_timeout: Duration,
}

impl TransportConfig {
    pub fn new(local: Hello) -> Self {
        Self {
            local,
            keepalive: KeepAlive::default(),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

/// Start the manager. Abort the returned handle (or drop the runtime) to stop.
pub fn spawn(
    cfg: TransportConfig,
    listener: TcpListener,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    events: Option<mpsc::Sender<TransportEvent>>,
) -> JoinHandle<()> {
    tokio::spawn(manager(cfg, listener, discovery_rx, events))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Dialer,
    Acceptor,
}

fn role_for(own_id: &str, peer_id: &str) -> Role {
    if own_id < peer_id {
        Role::Dialer
    } else {
        Role::Acceptor
    }
}

/// Messages folded into the manager's single processing loop.
enum Internal {
    Discovery(DiscoveryEvent),
    /// An inbound connection cleared its handshake.
    InboundReady {
        peer: Hello,
        caps: Caps,
        stream: TcpStream,
    },
    /// A connection (either role) became live.
    ConnUp {
        peer_id: String,
        device_name: String,
        caps: Caps,
    },
    /// A connection ended.
    ConnDown {
        peer_id: String,
        reason: DisconnectReason,
    },
}

struct PeerEntry {
    role: Role,
    device_name: String,
    connected: bool,
    /// Present iff we are the dialer: pushes fresh address lists to the loop.
    addrs_tx: Option<watch::Sender<Vec<SocketAddr>>>,
    /// The dialer reconnect loop, or the acceptor's active connection task.
    task: Option<JoinHandle<()>>,
}

impl PeerEntry {
    fn new(role: Role) -> Self {
        Self {
            role,
            device_name: String::new(),
            connected: false,
            addrs_tx: None,
            task: None,
        }
    }
}

async fn manager(
    cfg: TransportConfig,
    listener: TcpListener,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    events: Option<mpsc::Sender<TransportEvent>>,
) {
    let own_id = cfg.local.device_id.clone();
    let (internal_tx, mut internal_rx) = mpsc::channel::<Internal>(64);

    spawn_accept_loop(
        listener,
        cfg.local.clone(),
        cfg.keepalive,
        internal_tx.clone(),
    );
    spawn_discovery_forwarder(discovery_rx, internal_tx.clone());

    let mut peers: HashMap<String, PeerEntry> = HashMap::new();

    while let Some(msg) = internal_rx.recv().await {
        match msg {
            Internal::Discovery(DiscoveryEvent::Found(p)) => {
                let role = role_for(&own_id, &p.device_id);
                let entry = peers
                    .entry(p.device_id.clone())
                    .or_insert_with(|| PeerEntry::new(role));
                entry.device_name = p.device_name.clone();

                match role {
                    Role::Dialer => {
                        if let Some(tx) = &entry.addrs_tx {
                            let _ = tx.send(p.addrs.clone());
                        } else {
                            let (tx, rx) = watch::channel(p.addrs.clone());
                            entry.addrs_tx = Some(tx);
                            let task = tokio::spawn(dial_loop(
                                p.device_id.clone(),
                                cfg.clone(),
                                rx,
                                internal_tx.clone(),
                            ));
                            entry.task = Some(task);
                            tracing::info!(peer = %p.device_id, name = %p.device_name, "discovered peer (we dial)");
                        }
                    }
                    Role::Acceptor => {
                        tracing::info!(peer = %p.device_id, name = %p.device_name, "discovered peer (we accept)");
                    }
                }
            }
            Internal::Discovery(DiscoveryEvent::Lost { device_id }) => {
                if let Some(entry) = peers.get_mut(&device_id) {
                    tracing::info!(peer = %device_id, "peer lost from mDNS");
                    // Pause the reconnect loop but leave a live connection alone
                    // until keepalive proves it dead.
                    if let Some(tx) = &entry.addrs_tx {
                        let _ = tx.send(Vec::new());
                    }
                }
            }
            Internal::InboundReady { peer, caps, stream } => {
                let peer_id = peer.device_id.clone();
                if role_for(&own_id, &peer_id) == Role::Dialer {
                    tracing::debug!(peer = %peer_id, "rejecting inbound: we are the dialer for this peer");
                    continue;
                }
                let entry = peers
                    .entry(peer_id.clone())
                    .or_insert_with(|| PeerEntry::new(Role::Acceptor));
                entry.device_name = peer.device_name.clone();

                if let Some(old) = entry.task.take() {
                    old.abort();
                    if entry.connected {
                        entry.connected = false;
                        emit(
                            &events,
                            TransportEvent::PeerDisconnected {
                                device_id: peer_id.clone(),
                                reason: "replaced by a newer inbound connection".into(),
                            },
                        )
                        .await;
                    }
                }

                let ka = cfg.keepalive;
                let tx = internal_tx.clone();
                let pid = peer_id.clone();
                let name = peer.device_name.clone();
                let task = tokio::spawn(async move {
                    let _ = tx
                        .send(Internal::ConnUp {
                            peer_id: pid.clone(),
                            device_name: name,
                            caps,
                        })
                        .await;
                    let reason = connection::run(stream, ka).await;
                    let _ = tx
                        .send(Internal::ConnDown {
                            peer_id: pid,
                            reason,
                        })
                        .await;
                });
                entry.task = Some(task);
            }
            Internal::ConnUp {
                peer_id,
                device_name,
                caps,
            } => {
                if let Some(entry) = peers.get_mut(&peer_id) {
                    entry.connected = true;
                    entry.device_name = device_name.clone();
                }
                tracing::info!(peer = %peer_id, caps = ?caps, "peer connected");
                emit(
                    &events,
                    TransportEvent::PeerConnected {
                        device_id: peer_id,
                        device_name,
                        caps,
                    },
                )
                .await;
            }
            Internal::ConnDown { peer_id, reason } => {
                if let Some(entry) = peers.get_mut(&peer_id) {
                    entry.connected = false;
                    if entry.role == Role::Acceptor {
                        entry.task = None;
                    }
                }
                tracing::info!(peer = %peer_id, %reason, "peer disconnected");
                emit(
                    &events,
                    TransportEvent::PeerDisconnected {
                        device_id: peer_id,
                        reason: reason.to_string(),
                    },
                )
                .await;
            }
        }
    }
}

fn spawn_accept_loop(
    listener: TcpListener,
    local: Hello,
    ka: KeepAlive,
    internal_tx: mpsc::Sender<Internal>,
) {
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    let _ = stream.set_nodelay(true);
                    let local = local.clone();
                    let internal_tx = internal_tx.clone();
                    tokio::spawn(async move {
                        let mut stream = stream;
                        match handshake(&mut stream, &local, None, ka.handshake_timeout).await {
                            Ok(hs) => {
                                let _ = internal_tx
                                    .send(Internal::InboundReady {
                                        peer: hs.peer,
                                        caps: hs.effective_caps,
                                        stream,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                tracing::debug!(%addr, "inbound handshake failed: {e}");
                            }
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
}

fn spawn_discovery_forwarder(
    mut discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    internal_tx: mpsc::Sender<Internal>,
) {
    tokio::spawn(async move {
        while let Some(ev) = discovery_rx.recv().await {
            if internal_tx.send(Internal::Discovery(ev)).await.is_err() {
                break;
            }
        }
    });
}

/// The dialer's connect/keepalive/reconnect loop for one peer.
async fn dial_loop(
    peer_id: String,
    cfg: TransportConfig,
    mut addrs_rx: watch::Receiver<Vec<SocketAddr>>,
    internal_tx: mpsc::Sender<Internal>,
) {
    let mut backoff = Backoff::new(cfg.backoff_base, cfg.backoff_max);

    loop {
        let addrs = addrs_rx.borrow_and_update().clone();
        if addrs.is_empty() {
            if addrs_rx.changed().await.is_err() {
                return;
            }
            continue;
        }

        let established = dial_once(&addrs, &peer_id, &cfg, &internal_tx).await;
        if established {
            // The connection ran and dropped; try to restore it promptly.
            backoff.reset();
        }

        let delay = backoff.next_delay();
        tracing::debug!(peer = %peer_id, ?delay, "backing off before next dial");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            r = addrs_rx.changed() => {
                if r.is_err() {
                    return;
                }
            }
        }
    }
}

/// One pass over a peer's candidate addresses. Returns `true` if a connection
/// was established (and has since ended), `false` if none could be made.
async fn dial_once(
    addrs: &[SocketAddr],
    peer_id: &str,
    cfg: &TransportConfig,
    internal_tx: &mpsc::Sender<Internal>,
) -> bool {
    for &addr in addrs {
        let stream = match timeout(cfg.connect_timeout, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::debug!(peer = %peer_id, %addr, "connect failed: {e}");
                continue;
            }
            Err(_) => {
                tracing::debug!(peer = %peer_id, %addr, "connect timed out");
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let mut stream = stream;

        let hs = match handshake(
            &mut stream,
            &cfg.local,
            Some(peer_id),
            cfg.keepalive.handshake_timeout,
        )
        .await
        {
            Ok(hs) => hs,
            Err(e) => {
                tracing::warn!(peer = %peer_id, %addr, "handshake failed: {e}");
                continue;
            }
        };

        let _ = internal_tx
            .send(Internal::ConnUp {
                peer_id: peer_id.to_string(),
                device_name: hs.peer.device_name.clone(),
                caps: hs.effective_caps,
            })
            .await;
        let reason = connection::run(stream, cfg.keepalive).await;
        let _ = internal_tx
            .send(Internal::ConnDown {
                peer_id: peer_id.to_string(),
                reason,
            })
            .await;
        return true;
    }
    false
}

async fn emit(events: &Option<mpsc::Sender<TransportEvent>>, ev: TransportEvent) {
    if let Some(tx) = events {
        let _ = tx.send(ev).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdrop_core::proto::PROTOCOL_VERSION;

    fn hello(id: &str) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: id.into(),
            device_name: format!("dev-{id}"),
            caps: Caps::ALL,
        }
    }

    fn fast_cfg(local: Hello) -> TransportConfig {
        TransportConfig {
            local,
            keepalive: KeepAlive {
                interval: Duration::from_millis(100),
                idle_timeout: Duration::from_millis(500),
                handshake_timeout: Duration::from_secs(2),
            },
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(100),
            connect_timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn arbitration_is_deterministic_and_opposite() {
        assert_eq!(role_for("aaaa", "ffff"), Role::Dialer);
        assert_eq!(role_for("ffff", "aaaa"), Role::Acceptor);
    }

    // As the low id, the manager should dial the discovered peer, and it
    // should re-dial after the connection drops.
    #[tokio::test]
    async fn dialer_connects_and_reconnects() {
        let peer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer_listener.local_addr().unwrap();

        let own = hello("aaaa0000000000000000000000000000");
        let cfg = fast_cfg(own);

        let our_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (disco_tx, disco_rx) = mpsc::channel(8);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let mgr = spawn(cfg, our_listener, disco_rx, Some(ev_tx));

        disco_tx
            .send(DiscoveryEvent::Found(crate::discovery::DiscoveredPeer {
                device_id: "ffff1111111111111111111111111111".into(),
                device_name: "peer".into(),
                fingerprint: "ffff1111111111111111111111111111".into(),
                addrs: vec![peer_addr],
            }))
            .await
            .unwrap();

        for round in 0..2 {
            let (mut sock, _) = peer_listener.accept().await.unwrap();
            let peer_hello = hello("ffff1111111111111111111111111111");
            let hs = handshake(&mut sock, &peer_hello, None, Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(hs.peer.device_id, "aaaa0000000000000000000000000000");

            match tokio::time::timeout(Duration::from_secs(2), ev_rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                TransportEvent::PeerConnected { device_id, .. } => {
                    assert_eq!(device_id, "ffff1111111111111111111111111111");
                }
                other => panic!("round {round}: expected connect, got {other:?}"),
            }

            // Drop the peer socket to force a disconnect + reconnect.
            drop(sock);
            match tokio::time::timeout(Duration::from_secs(2), ev_rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                TransportEvent::PeerDisconnected { device_id, .. } => {
                    assert_eq!(device_id, "ffff1111111111111111111111111111");
                }
                other => panic!("round {round}: expected disconnect, got {other:?}"),
            }
        }

        mgr.abort();
    }

    // As the high id, the manager must NOT dial; it accepts an inbound
    // connection instead.
    #[tokio::test]
    async fn acceptor_does_not_dial_but_accepts_inbound() {
        let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_addr = unused.local_addr().unwrap();

        let own = hello("ffff2222222222222222222222222222");
        let cfg = fast_cfg(own);
        let our_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let our_addr = our_listener.local_addr().unwrap();

        let (disco_tx, disco_rx) = mpsc::channel(8);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let mgr = spawn(cfg, our_listener, disco_rx, Some(ev_tx));

        disco_tx
            .send(DiscoveryEvent::Found(crate::discovery::DiscoveredPeer {
                device_id: "aaaa3333333333333333333333333333".into(),
                device_name: "peer".into(),
                fingerprint: "aaaa3333333333333333333333333333".into(),
                addrs: vec![unused_addr],
            }))
            .await
            .unwrap();

        // It must not dial the discovered address.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), unused.accept())
                .await
                .is_err(),
            "acceptor should not have dialed"
        );

        // Now connect to it ourselves, as the lower-id peer would.
        let mut sock = TcpStream::connect(our_addr).await.unwrap();
        let peer_hello = hello("aaaa3333333333333333333333333333");
        handshake(&mut sock, &peer_hello, None, Duration::from_secs(2))
            .await
            .unwrap();

        match tokio::time::timeout(Duration::from_secs(2), ev_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            TransportEvent::PeerConnected { device_id, .. } => {
                assert_eq!(device_id, "aaaa3333333333333333333333333333");
            }
            other => panic!("expected connect, got {other:?}"),
        }

        drop(sock);
        mgr.abort();
    }
}
