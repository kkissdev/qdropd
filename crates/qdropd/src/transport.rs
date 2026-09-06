//! Connection manager: turns discovery events into exactly one live,
//! TLS-authenticated connection per *paired* peer.
//!
//! Arbitration: the device with the lexicographically **lower** id dials; the
//! other only accepts. Only peers in the roster (paired in M2) are dialed,
//! and inbound TLS from an unpinned key is refused by the verifier. The
//! roster can change at runtime (pair / unpair) without a restart.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use qdrop_core::identity::Identity;
use qdrop_core::proto::{Caps, Hello, Message};
use qdrop_core::roster::Roster;
use rustls::ServerConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::backoff::Backoff;
use crate::bus::PeerBus;
use crate::connection::{self, handshake, DisconnectReason, KeepAlive};
use crate::discovery::{DiscoveredPeer, DiscoveryEvent};
use crate::secure;

/// Inbound application frames from any peer: `(peer_id, message)`.
pub type Hub = mpsc::Sender<(String, Message)>;

/// Observability hook — one event per connection state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    PeerConnected {
        device_id: String,
        device_name: String,
        caps: Caps,
        /// The address we dialed, when we were the dialer.
        addr: Option<String>,
    },
    PeerDisconnected {
        device_id: String,
        reason: String,
    },
}

/// Tuning + shared state for the manager.
#[derive(Clone)]
pub struct TransportConfig {
    /// Our own `Hello` (identity + advertised caps).
    pub local: Hello,
    pub identity: Arc<Identity>,
    pub roster: Roster,
    pub server_config: Arc<ServerConfig>,
    pub keepalive: KeepAlive,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub connect_timeout: Duration,
}

impl TransportConfig {
    pub fn new(
        local: Hello,
        identity: Arc<Identity>,
        roster: Roster,
        server_config: Arc<ServerConfig>,
    ) -> Self {
        Self {
            local,
            identity,
            roster,
            server_config,
            keepalive: KeepAlive::default(),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
        }
    }
}

/// A watch receiver whose sender is already gone, so `changed()` returns
/// immediately and the roster-reload path stays dormant (used in tests).
#[cfg(test)]
fn never() -> watch::Receiver<()> {
    watch::channel(()).1
}

/// Start the manager. Abort the returned handle (or drop the runtime) to stop.
/// `roster_changed` fires after the shared roster is reloaded from disk.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    cfg: TransportConfig,
    listener: TcpListener,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    roster_changed: watch::Receiver<()>,
    bus: PeerBus,
    hub: Hub,
    events: Option<mpsc::Sender<TransportEvent>>,
) -> JoinHandle<()> {
    tokio::spawn(manager(
        cfg,
        listener,
        discovery_rx,
        roster_changed,
        bus,
        hub,
        events,
    ))
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
    RosterChanged,
    InboundReady {
        peer: Hello,
        caps: Caps,
        stream: Box<secure::SecureStream>,
    },
    ConnUp {
        peer_id: String,
        device_name: String,
        caps: Caps,
        addr: Option<String>,
        outbound: mpsc::Sender<Message>,
    },
    ConnDown {
        peer_id: String,
        reason: DisconnectReason,
    },
}

/// Wire up a freshly-handshaked stream: announce it, run it, announce its end.
#[allow(clippy::too_many_arguments)]
async fn drive_connection<S>(
    stream: S,
    peer_id: String,
    device_name: String,
    caps: Caps,
    addr: Option<String>,
    ka: KeepAlive,
    hub: Hub,
    internal_tx: mpsc::Sender<Internal>,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (out_tx, out_rx) = mpsc::channel::<Message>(64);
    let _ = internal_tx
        .send(Internal::ConnUp {
            peer_id: peer_id.clone(),
            device_name,
            caps,
            addr,
            outbound: out_tx,
        })
        .await;
    let wire = connection::Wire {
        peer_id: peer_id.clone(),
        inbound: hub,
        outbound: out_rx,
    };
    let reason = connection::run(stream, ka, wire).await;
    let _ = internal_tx
        .send(Internal::ConnDown { peer_id, reason })
        .await;
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

#[allow(clippy::too_many_arguments)]
async fn manager(
    cfg: TransportConfig,
    listener: TcpListener,
    discovery_rx: mpsc::Receiver<DiscoveryEvent>,
    roster_changed: watch::Receiver<()>,
    bus: PeerBus,
    hub: Hub,
    events: Option<mpsc::Sender<TransportEvent>>,
) {
    let own_id = cfg.local.device_id.clone();
    let (internal_tx, mut internal_rx) = mpsc::channel::<Internal>(64);

    spawn_accept_loop(listener, cfg.clone(), internal_tx.clone());
    spawn_discovery_forwarder(discovery_rx, internal_tx.clone());
    spawn_roster_watcher(roster_changed, internal_tx.clone());

    let mut peers: HashMap<String, PeerEntry> = HashMap::new();
    // Everything mDNS has told us about, trusted or not, so we can start
    // dialing immediately when a peer becomes trusted.
    let mut discovered: HashMap<String, DiscoveredPeer> = HashMap::new();

    // Peers with an operator-configured `address` are dialed without waiting
    // for mDNS (they are typically on another link where it never resolves).
    for p in static_peers(&cfg) {
        if cfg.roster.read().is_trusted_id(&p.device_id) {
            ensure_peer(&cfg, &own_id, &internal_tx, &hub, &mut peers, &p);
        }
    }

    while let Some(msg) = internal_rx.recv().await {
        match msg {
            Internal::Discovery(DiscoveryEvent::Found(p)) => {
                discovered.insert(p.device_id.clone(), p.clone());
                if !cfg.roster.read().is_trusted_id(&p.device_id) {
                    tracing::debug!(peer = %p.device_id, "ignoring unpaired peer");
                    continue;
                }
                ensure_peer(&cfg, &own_id, &internal_tx, &hub, &mut peers, &p);
            }
            Internal::Discovery(DiscoveryEvent::Lost { device_id }) => {
                discovered.remove(&device_id);
                let fallback = cfg.roster.read().static_addrs_for(&device_id);
                if let Some(entry) = peers.get_mut(&device_id) {
                    if fallback.is_empty() {
                        tracing::info!(peer = %device_id, "peer lost from mDNS");
                    } else {
                        tracing::info!(peer = %device_id, "peer lost from mDNS; keeping static address");
                    }
                    if let Some(tx) = &entry.addrs_tx {
                        let _ = tx.send(fallback);
                    }
                }
            }
            Internal::RosterChanged => {
                // Drop connections to peers that are no longer trusted.
                let trusted = cfg.roster.read().ids();
                let dropped: Vec<String> = peers
                    .keys()
                    .filter(|id| !trusted.contains(id))
                    .cloned()
                    .collect();
                for id in dropped {
                    bus.remove(&id);
                    if let Some(mut entry) = peers.remove(&id) {
                        if let Some(t) = entry.task.take() {
                            t.abort();
                        }
                        if entry.connected {
                            emit(
                                &events,
                                TransportEvent::PeerDisconnected {
                                    device_id: id.clone(),
                                    reason: "peer unpaired".into(),
                                },
                            )
                            .await;
                        }
                        tracing::info!(peer = %id, "dropped connection: peer unpaired");
                    }
                }
                // Start dialing peers that just became trusted, whether we
                // know them from mDNS or only from a configured static address.
                let mut seed: HashMap<String, DiscoveredPeer> = discovered.clone();
                for p in static_peers(&cfg) {
                    seed.entry(p.device_id.clone()).or_insert(p);
                }
                for p in seed.into_values() {
                    if cfg.roster.read().is_trusted_id(&p.device_id) {
                        ensure_peer(&cfg, &own_id, &internal_tx, &hub, &mut peers, &p);
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

                let task = tokio::spawn(drive_connection(
                    stream,
                    peer_id.clone(),
                    peer.device_name.clone(),
                    caps,
                    None, // inbound: we did not dial
                    cfg.keepalive,
                    hub.clone(),
                    internal_tx.clone(),
                ));
                entry.task = Some(task);
            }
            Internal::ConnUp {
                peer_id,
                device_name,
                caps,
                addr,
                outbound,
            } => {
                if let Some(entry) = peers.get_mut(&peer_id) {
                    entry.connected = true;
                    entry.device_name = device_name.clone();
                }
                bus.insert(peer_id.clone(), outbound);
                tracing::info!(peer = %peer_id, caps = ?caps, addr = ?addr, "peer connected");
                emit(
                    &events,
                    TransportEvent::PeerConnected {
                        device_id: peer_id,
                        device_name,
                        caps,
                        addr,
                    },
                )
                .await;
            }
            Internal::ConnDown { peer_id, reason } => {
                bus.remove(&peer_id);
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

/// Ensure a dial loop exists (or its address book is refreshed) for a trusted,
/// discovered peer we are the dialer for.
fn ensure_peer(
    cfg: &TransportConfig,
    own_id: &str,
    internal_tx: &mpsc::Sender<Internal>,
    hub: &Hub,
    peers: &mut HashMap<String, PeerEntry>,
    p: &DiscoveredPeer,
) {
    let role = role_for(own_id, &p.device_id);
    let entry = peers
        .entry(p.device_id.clone())
        .or_insert_with(|| PeerEntry::new(role));
    entry.device_name = p.device_name.clone();

    match role {
        Role::Dialer => {
            let mut addrs = p.addrs.clone();
            for sa in cfg.roster.read().static_addrs_for(&p.device_id) {
                if !addrs.contains(&sa) {
                    addrs.push(sa);
                }
            }
            if let Some(tx) = &entry.addrs_tx {
                let _ = tx.send(addrs);
            } else {
                let (tx, rx) = watch::channel(addrs);
                entry.addrs_tx = Some(tx);
                entry.task = Some(tokio::spawn(dial_loop(
                    p.device_id.clone(),
                    cfg.clone(),
                    rx,
                    hub.clone(),
                    internal_tx.clone(),
                )));
                tracing::info!(peer = %p.device_id, name = %p.device_name, "discovered paired peer (we dial)");
            }
        }
        Role::Acceptor => {
            tracing::info!(peer = %p.device_id, name = %p.device_name, "discovered paired peer (we accept)");
        }
    }
}

/// Synthetic discovery records for roster peers that carry a static address.
fn static_peers(cfg: &TransportConfig) -> Vec<DiscoveredPeer> {
    cfg.roster
        .read()
        .peers_with_static_addrs()
        .into_iter()
        .map(|(device_id, device_name, addrs)| DiscoveredPeer {
            device_id,
            device_name,
            fingerprint: String::new(),
            addrs,
        })
        .collect()
}

fn spawn_accept_loop(
    listener: TcpListener,
    cfg: TransportConfig,
    internal_tx: mpsc::Sender<Internal>,
) {
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((tcp, addr)) => {
                    let _ = tcp.set_nodelay(true);
                    let cfg = cfg.clone();
                    let internal_tx = internal_tx.clone();
                    tokio::spawn(async move {
                        let ka = cfg.keepalive;
                        let (mut stream, key) = match secure::accept(
                            tcp,
                            cfg.server_config.clone(),
                            &cfg.roster,
                            ka.handshake_timeout,
                        )
                        .await
                        {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::debug!(%addr, "inbound TLS rejected: {e:#}");
                                return;
                            }
                        };
                        let expect_id = cfg
                            .roster
                            .read()
                            .peer_for_key(&key)
                            .map(|p| p.device_id.clone());
                        match handshake(
                            &mut stream,
                            &cfg.local,
                            expect_id.as_deref(),
                            ka.handshake_timeout,
                        )
                        .await
                        {
                            Ok(hs) => {
                                let _ = internal_tx
                                    .send(Internal::InboundReady {
                                        peer: hs.peer,
                                        caps: hs.effective_caps,
                                        stream: Box::new(stream),
                                    })
                                    .await;
                            }
                            Err(e) => tracing::debug!(%addr, "inbound handshake failed: {e}"),
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

fn spawn_roster_watcher(
    mut roster_changed: watch::Receiver<()>,
    internal_tx: mpsc::Sender<Internal>,
) {
    tokio::spawn(async move {
        while roster_changed.changed().await.is_ok() {
            if internal_tx.send(Internal::RosterChanged).await.is_err() {
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
    hub: Hub,
    internal_tx: mpsc::Sender<Internal>,
) {
    let mut backoff = Backoff::new(cfg.backoff_base, cfg.backoff_max);

    loop {
        let addrs = addrs_rx.borrow_and_update().clone();
        let expect = cfg.roster.read().key_for_id(&peer_id);
        match (addrs.is_empty(), expect) {
            (false, Some(expect)) => {
                if dial_once(&addrs, expect, &peer_id, &cfg, &hub, &internal_tx).await {
                    backoff.reset();
                }
            }
            _ => {
                if addrs_rx.changed().await.is_err() {
                    return;
                }
                continue;
            }
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
/// was established (and has since ended).
#[allow(clippy::too_many_arguments)]
async fn dial_once(
    addrs: &[SocketAddr],
    expect: [u8; 32],
    peer_id: &str,
    cfg: &TransportConfig,
    hub: &Hub,
    internal_tx: &mpsc::Sender<Internal>,
) -> bool {
    for &addr in addrs {
        let tcp = match timeout(cfg.connect_timeout, TcpStream::connect(addr)).await {
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
        let _ = tcp.set_nodelay(true);

        let mut stream = match secure::connect(
            tcp,
            &cfg.identity,
            expect,
            cfg.keepalive.handshake_timeout,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(peer = %peer_id, %addr, "TLS handshake failed: {e:#}");
                continue;
            }
        };

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

        drive_connection(
            stream,
            peer_id.to_string(),
            hs.peer.device_name.clone(),
            hs.effective_caps,
            Some(addr.to_string()),
            cfg.keepalive,
            hub.clone(),
            internal_tx.clone(),
        )
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
    use qdrop_core::{Peer, Peers};
    use std::path::Path;

    fn hello(id: &str) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: id.into(),
            device_name: format!("dev-{id}"),
            caps: Caps::ALL,
        }
    }

    fn identity_at(dir: &Path, name: &str) -> Arc<Identity> {
        Arc::new(Identity::load_or_create_at(dir.join(name)).unwrap())
    }

    fn fast_cfg(local: Hello, identity: Arc<Identity>, roster: Roster) -> TransportConfig {
        let server_config = qdrop_core::tls::server_config(&identity, roster.clone()).unwrap();
        TransportConfig {
            local,
            identity,
            roster,
            server_config,
            keepalive: KeepAlive {
                interval: Duration::from_millis(120),
                idle_timeout: Duration::from_millis(600),
                handshake_timeout: Duration::from_secs(3),
            },
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(120),
            connect_timeout: Duration::from_secs(2),
        }
    }

    fn roster_of(peers: &[(&str, &Identity)]) -> Roster {
        let mut doc = Peers::default();
        for (id, ident) in peers {
            doc.upsert(Peer::new(format!("name-{id}"), *id, &ident.public_key()));
        }
        Roster::from_peers(&doc)
    }

    fn roster_with_addr(id: &str, ident: &Identity, addr: SocketAddr) -> Roster {
        let mut doc = Peers::default();
        let mut p = Peer::new(format!("name-{id}"), id, &ident.public_key());
        p.address = Some(addr.to_string());
        doc.upsert(p);
        Roster::from_peers(&doc)
    }

    #[test]
    fn arbitration_is_deterministic_and_opposite() {
        assert_eq!(role_for("aaaa", "ffff"), Role::Dialer);
        assert_eq!(role_for("ffff", "aaaa"), Role::Acceptor);
    }

    const LOW: &str = "aaaa0000000000000000000000000000";
    const HIGH: &str = "ffff1111111111111111111111111111";

    // The low-id side dials a paired peer over TLS, and re-dials after a drop.
    #[tokio::test]
    async fn dialer_connects_over_tls_and_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let us = identity_at(dir.path(), "us.pem");
        let them = identity_at(dir.path(), "them.pem");

        // Peer side: a TLS server that trusts us.
        let peer_roster = roster_of(&[(LOW, &us)]);
        let peer_server = qdrop_core::tls::server_config(&them, peer_roster.clone()).unwrap();
        let peer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer_listener.local_addr().unwrap();

        let our_roster = roster_of(&[(HIGH, &them)]);
        let cfg = fast_cfg(hello(LOW), us.clone(), our_roster);
        let our_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (disco_tx, disco_rx) = mpsc::channel(8);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let (hub_tx, _hub_rx) = mpsc::channel(16);
        let mgr = spawn(
            cfg,
            our_listener,
            disco_rx,
            never(),
            PeerBus::new(),
            hub_tx,
            Some(ev_tx),
        );

        disco_tx
            .send(DiscoveryEvent::Found(DiscoveredPeer {
                device_id: HIGH.into(),
                device_name: "peer".into(),
                fingerprint: "fp".into(),
                addrs: vec![peer_addr],
            }))
            .await
            .unwrap();

        for round in 0..2 {
            let (tcp, _) = peer_listener.accept().await.unwrap();
            let acceptor = tokio_rustls::TlsAcceptor::from(peer_server.clone());
            let mut tls = secure::SecureStream::from(acceptor.accept(tcp).await.unwrap());
            let peer_hello = hello(HIGH);
            let hs = handshake(&mut tls, &peer_hello, Some(LOW), Duration::from_secs(3))
                .await
                .unwrap();
            assert_eq!(hs.peer.device_id, LOW);

            match tokio::time::timeout(Duration::from_secs(3), ev_rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                TransportEvent::PeerConnected { device_id, .. } => assert_eq!(device_id, HIGH),
                other => panic!("round {round}: expected connect, got {other:?}"),
            }

            drop(tls);
            match tokio::time::timeout(Duration::from_secs(3), ev_rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                TransportEvent::PeerDisconnected { device_id, .. } => assert_eq!(device_id, HIGH),
                other => panic!("round {round}: expected disconnect, got {other:?}"),
            }
        }
        mgr.abort();
    }

    // A paired peer with a static `address` is dialed even though mDNS never
    // resolves it (no discovery event is ever sent).
    #[tokio::test]
    async fn static_address_is_dialed_without_mdns() {
        let dir = tempfile::tempdir().unwrap();
        let us = identity_at(dir.path(), "us.pem");
        let them = identity_at(dir.path(), "them.pem");

        let peer_roster = roster_of(&[(LOW, &us)]);
        let peer_server = qdrop_core::tls::server_config(&them, peer_roster).unwrap();
        let peer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer_listener.local_addr().unwrap();

        let our_roster = roster_with_addr(HIGH, &them, peer_addr);
        let cfg = fast_cfg(hello(LOW), us.clone(), our_roster);
        let our_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (_disco_tx, disco_rx) = mpsc::channel(8);
        let (ev_tx, mut ev_rx) = mpsc::channel(16);
        let (hub_tx, _hub_rx) = mpsc::channel(16);
        let mgr = spawn(
            cfg,
            our_listener,
            disco_rx,
            never(),
            PeerBus::new(),
            hub_tx,
            Some(ev_tx),
        );

        let (tcp, _) = tokio::time::timeout(Duration::from_secs(3), peer_listener.accept())
            .await
            .expect("dialer should connect via the static address")
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(peer_server);
        let mut tls = secure::SecureStream::from(acceptor.accept(tcp).await.unwrap());
        let peer_hello = hello(HIGH);
        handshake(&mut tls, &peer_hello, Some(LOW), Duration::from_secs(3))
            .await
            .unwrap();

        match tokio::time::timeout(Duration::from_secs(3), ev_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            TransportEvent::PeerConnected { device_id, .. } => assert_eq!(device_id, HIGH),
            other => panic!("expected connect, got {other:?}"),
        }
        mgr.abort();
    }

    // An unpaired peer discovered over mDNS is never dialed.
    #[tokio::test]
    async fn unpaired_peer_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let us = identity_at(dir.path(), "us.pem");
        let them = identity_at(dir.path(), "them.pem");

        let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_addr = unused.local_addr().unwrap();

        // Empty roster: nobody is trusted.
        let cfg = fast_cfg(hello(LOW), us.clone(), Roster::new());
        let our_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (disco_tx, disco_rx) = mpsc::channel(8);
        let (hub_tx, _hub_rx) = mpsc::channel(16);
        let mgr = spawn(
            cfg,
            our_listener,
            disco_rx,
            never(),
            PeerBus::new(),
            hub_tx,
            None,
        );

        disco_tx
            .send(DiscoveryEvent::Found(DiscoveredPeer {
                device_id: HIGH.into(),
                device_name: "stranger".into(),
                fingerprint: "fp".into(),
                addrs: vec![unused_addr],
            }))
            .await
            .unwrap();

        assert!(
            tokio::time::timeout(Duration::from_millis(400), unused.accept())
                .await
                .is_err(),
            "must not dial an unpaired peer"
        );
        let _ = them;
        mgr.abort();
    }
}
