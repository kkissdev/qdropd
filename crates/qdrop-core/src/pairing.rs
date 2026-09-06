//! PIN-authenticated pairing (SPAKE2), and the transient `_qdrop-pair._tcp`
//! rendezvous used to bootstrap it.
//!
//! Flow:
//! 1. Side A runs `qdrop pair`, which prints a 6-digit PIN, advertises
//!    `_qdrop-pair._tcp` on [`PAIR_PORT`], and waits up to 60s for a dialer.
//! 2. Side B runs `qdrop pair <name-or-ip>`, is prompted for the PIN, connects,
//!    and both sides run SPAKE2 keyed by the PIN. The PIN never crosses the
//!    wire.
//! 3. Over the SPAKE2-derived key each side sends its identity public key,
//!    device id and name, authenticated with HMAC. A wrong PIN or a MITM makes
//!    the HMAC check fail and pairing aborts before anything is trusted.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity as SpakeId, Password, Spake2};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::crypto::{ct_eq, hmac_sha256};
use crate::identity::Identity;
use crate::{DEFAULT_PORT, PAIR_SERVICE_TYPE};

/// TCP port side A listens on for pairing (also advertised via mDNS).
pub const PAIR_PORT: u16 = DEFAULT_PORT + 1;

/// How long side A stays discoverable / waits for a dialer.
pub const OFFER_WINDOW: Duration = Duration::from_secs(60);

const SPAKE_ID_A: &[u8] = b"qdrop-pair-offerer";
const SPAKE_ID_B: &[u8] = b"qdrop-pair-requester";
const MAX_PAIR_FRAME: usize = 4096;

/// Which side of the exchange we are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `qdrop pair` — printed the PIN, listening.
    Offerer,
    /// `qdrop pair <target>` — typed the PIN, dialing.
    Requester,
}

/// What we learned about the peer during a successful pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub device_id: String,
    pub device_name: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error("no device matching {0:?} is offering to pair on this network")]
    TargetNotFound(String),
    #[error("timed out waiting for the other device")]
    Timeout,
    #[error("pairing failed: wrong PIN or a network tamper (key confirmation failed)")]
    BadPin,
    #[error("the other device aborted pairing")]
    PeerAborted,
    #[error("protocol error during pairing: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Serialize, Deserialize)]
struct PairPayload {
    device_id: String,
    device_name: String,
    public_key: Vec<u8>,
}

/// Our own side's inputs to the exchange.
pub struct LocalInfo {
    pub device_id: String,
    pub device_name: String,
    pub public_key: [u8; 32],
}

impl LocalInfo {
    pub fn new(identity: &Identity, device_id: &str, device_name: &str) -> Self {
        Self {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            public_key: identity.public_key(),
        }
    }
}

/// Side A: advertise, wait for a dialer, run the exchange. `announce_pin` is
/// called once with the freshly generated PIN so the caller can display it.
pub async fn run_offer(
    local: LocalInfo,
    announce_pin: impl FnOnce(&str),
) -> Result<Paired, PairError> {
    let pin = crate::crypto::gen_pin().map_err(PairError::Other)?;
    announce_pin(&pin);

    let listener = TcpListener::bind(SocketAddr::from((
        std::net::Ipv4Addr::UNSPECIFIED,
        PAIR_PORT,
    )))
    .await
    .with_context(|| format!("binding pairing port {PAIR_PORT}"))?;

    let advert = PairAdvertisement::start(&local.device_id, &local.device_name)?;

    let accepted = timeout(OFFER_WINDOW, listener.accept()).await;
    advert.stop();
    let (mut stream, _addr) = accepted.map_err(|_| PairError::Timeout)??;
    stream.set_nodelay(true).ok();

    exchange(&mut stream, Role::Offerer, &pin, &local).await
}

/// Side B: resolve the target, connect, prompt-provided `pin`, run the exchange.
pub async fn run_request(local: LocalInfo, target: &str, pin: &str) -> Result<Paired, PairError> {
    let addr = resolve_target(target).await?;
    let mut stream = timeout(Duration::from_secs(10), TcpStream::connect(addr))
        .await
        .map_err(|_| PairError::Timeout)?
        .with_context(|| format!("connecting to {addr}"))?;
    stream.set_nodelay(true).ok();

    exchange(&mut stream, Role::Requester, pin, &local).await
}

/// The PIN-keyed exchange over an established byte stream. Split out so it can
/// be tested over an in-memory duplex.
pub async fn exchange<S>(
    stream: &mut S,
    role: Role,
    pin: &str,
    local: &LocalInfo,
) -> Result<Paired, PairError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // 1. SPAKE2.
    let (state, our_spake) = match role {
        Role::Offerer => Spake2::<Ed25519Group>::start_a(
            &Password::new(pin.as_bytes()),
            &SpakeId::new(SPAKE_ID_A),
            &SpakeId::new(SPAKE_ID_B),
        ),
        Role::Requester => Spake2::<Ed25519Group>::start_b(
            &Password::new(pin.as_bytes()),
            &SpakeId::new(SPAKE_ID_A),
            &SpakeId::new(SPAKE_ID_B),
        ),
    };
    write_chunk(stream, &our_spake).await?;
    let their_spake = read_chunk(stream).await?;
    let key = state
        .finish(&their_spake)
        .map_err(|_| PairError::Protocol("SPAKE2 finish failed".into()))?;

    // 2. Direction-separated subkeys.
    let (send_key, recv_key) = match role {
        Role::Offerer => (derive(&key, b"a2b"), derive(&key, b"b2a")),
        Role::Requester => (derive(&key, b"b2a"), derive(&key, b"a2b")),
    };

    // 3. Authenticated identity exchange.
    let payload = rmp_serde::to_vec_named(&PairPayload {
        device_id: local.device_id.clone(),
        device_name: local.device_name.clone(),
        public_key: local.public_key.to_vec(),
    })
    .map_err(|e| PairError::Protocol(e.to_string()))?;
    write_chunk(stream, &payload).await?;
    write_exact(stream, &hmac_sha256(&send_key, &payload)).await?;

    let their_payload = read_chunk(stream).await?;
    let mut their_tag = [0u8; 32];
    read_into(stream, &mut their_tag).await?;
    if !ct_eq(&hmac_sha256(&recv_key, &their_payload), &their_tag) {
        // Tell the peer to stop waiting on our confirmation, then fail.
        let _ = write_exact(stream, &[0u8]).await;
        return Err(PairError::BadPin);
    }

    // 4. Mutual confirmation: only trust the peer once we know it also
    //    verified us. `1` = ok, `0` / EOF = abort.
    write_exact(stream, &[1u8]).await?;
    let mut ack = [0u8; 1];
    match timeout(Duration::from_secs(15), read_into(stream, &mut ack)).await {
        Ok(Ok(())) if ack[0] == 1 => {}
        _ => return Err(PairError::PeerAborted),
    }

    let parsed: PairPayload =
        rmp_serde::from_slice(&their_payload).map_err(|e| PairError::Protocol(e.to_string()))?;
    let public_key: [u8; 32] = parsed
        .public_key
        .as_slice()
        .try_into()
        .map_err(|_| PairError::Protocol("peer sent a malformed public key".into()))?;

    Ok(Paired {
        device_id: parsed.device_id,
        device_name: parsed.device_name,
        public_key,
    })
}

fn derive(key: &[u8], label: &[u8]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(label.len() + 1 + key.len());
    buf.extend_from_slice(label);
    buf.push(0);
    buf.extend_from_slice(key);
    crate::crypto::sha256(&buf)
}

// --- tiny length-prefixed framing (u16 prefix, distinct from the main proto) -

async fn write_chunk<S: AsyncWrite + Unpin>(s: &mut S, data: &[u8]) -> Result<(), PairError> {
    let len =
        u16::try_from(data.len()).map_err(|_| PairError::Protocol("chunk too large".into()))?;
    s.write_all(&len.to_be_bytes()).await?;
    s.write_all(data).await?;
    s.flush().await?;
    Ok(())
}

async fn write_exact<S: AsyncWrite + Unpin>(s: &mut S, data: &[u8]) -> Result<(), PairError> {
    s.write_all(data).await?;
    s.flush().await?;
    Ok(())
}

async fn read_chunk<S: AsyncRead + Unpin>(s: &mut S) -> Result<Vec<u8>, PairError> {
    let mut len = [0u8; 2];
    read_into(s, &mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_PAIR_FRAME {
        return Err(PairError::Protocol(format!("bad chunk length {len}")));
    }
    let mut buf = vec![0u8; len];
    read_into(s, &mut buf).await?;
    Ok(buf)
}

async fn read_into<S: AsyncRead + Unpin>(s: &mut S, buf: &mut [u8]) -> Result<(), PairError> {
    match s.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(PairError::PeerAborted),
        Err(e) => Err(e.into()),
    }
}

// --- mDNS rendezvous --------------------------------------------------------

struct PairAdvertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl PairAdvertisement {
    fn start(device_id: &str, device_name: &str) -> Result<Self, PairError> {
        let daemon = ServiceDaemon::new().context("starting mDNS for pairing")?;
        let mut txt = HashMap::new();
        txt.insert("id".to_string(), device_id.to_string());
        txt.insert("name".to_string(), device_name.to_string());
        let host = format!("{device_id}.local.");
        let info = ServiceInfo::new(PAIR_SERVICE_TYPE, device_id, &host, "", PAIR_PORT, txt)
            .context("building pairing service info")?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon
            .register(info)
            .context("registering pairing service")?;
        Ok(Self { daemon, fullname })
    }

    fn stop(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv();
        }
        let _ = self.daemon.shutdown();
    }
}

async fn resolve_target(target: &str) -> Result<SocketAddr, PairError> {
    if let Ok(ip) = target.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, PAIR_PORT));
    }

    let daemon = ServiceDaemon::new().context("starting mDNS browse for pairing")?;
    let rx = daemon
        .browse(PAIR_SERVICE_TYPE)
        .context("browsing for pairing services")?;

    let found = timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv_async().await {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let name_matches = info
                        .txt_properties
                        .get_property_val_str("name")
                        .map(|n| n.eq_ignore_ascii_case(target))
                        .unwrap_or(false);
                    let instance = info
                        .fullname
                        .strip_suffix(&format!(".{PAIR_SERVICE_TYPE}"))
                        .unwrap_or("");
                    if name_matches || instance.eq_ignore_ascii_case(target) {
                        if let Some(ip) = info.addresses.iter().next() {
                            return Some(SocketAddr::new(ip.to_ip_addr(), info.port));
                        }
                    }
                }
                Ok(_) => {}
                Err(_) => return None,
            }
        }
    })
    .await;

    let _ = daemon.shutdown();
    match found {
        Ok(Some(addr)) => Ok(addr),
        _ => Err(PairError::TargetNotFound(target.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(id: &str) -> LocalInfo {
        LocalInfo {
            device_id: id.to_string(),
            device_name: format!("dev-{id}"),
            public_key: {
                let mut k = [0u8; 32];
                k[0] = id.as_bytes()[0];
                k
            },
        }
    }

    #[tokio::test]
    async fn matching_pins_pair_successfully() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let la = local("a");
        let lb = local("b");
        let ta = tokio::spawn(async move { exchange(&mut a, Role::Offerer, "123456", &la).await });
        let rb = exchange(&mut b, Role::Requester, "123456", &lb)
            .await
            .unwrap();
        let ra = ta.await.unwrap().unwrap();

        assert_eq!(rb.device_id, "a");
        assert_eq!(ra.device_id, "b");
        assert_eq!(ra.public_key, local("b").public_key);
    }

    #[tokio::test]
    async fn mismatched_pins_fail_closed() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let la = local("a");
        let lb = local("b");
        let ta = tokio::spawn(async move { exchange(&mut a, Role::Offerer, "111111", &la).await });
        let err = exchange(&mut b, Role::Requester, "222222", &lb)
            .await
            .unwrap_err();
        assert!(matches!(err, PairError::BadPin | PairError::PeerAborted));
        assert!(ta.await.unwrap().is_err());
    }

    // Correct PIN on both sides, but an active attacker flips a byte of the
    // offerer's identity payload in transit. The HMAC over the payload (keyed
    // by the SPAKE2 secret) must make pairing fail closed.
    #[tokio::test]
    async fn tampered_payload_fails_closed() {
        let (off_stream, mid1) = tokio::io::duplex(8192);
        let (mid2, req_stream) = tokio::io::duplex(8192);
        let (mut mid1_rd, mut mid1_wr) = tokio::io::split(mid1);
        let (mut mid2_rd, mut mid2_wr) = tokio::io::split(mid2);

        let fwd = tokio::spawn(async move {
            forward_chunk(&mut mid1_rd, &mut mid2_wr, false).await; // SPAKE2
            forward_chunk(&mut mid1_rd, &mut mid2_wr, true).await; // payload (corrupted)
            let _ = tokio::io::copy(&mut mid1_rd, &mut mid2_wr).await;
        });
        let back = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut mid2_rd, &mut mid1_wr).await;
        });

        let la = local("a");
        let lb = local("b");
        let mut off_stream = off_stream;
        let mut req_stream = req_stream;
        let ta =
            tokio::spawn(
                async move { exchange(&mut off_stream, Role::Offerer, "424242", &la).await },
            );
        let rb = exchange(&mut req_stream, Role::Requester, "424242", &lb).await;

        assert!(rb.is_err(), "requester must reject a tampered payload");
        assert!(ta.await.unwrap().is_err());
        fwd.abort();
        back.abort();
    }

    async fn forward_chunk<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        r: &mut R,
        w: &mut W,
        corrupt: bool,
    ) {
        let mut len = [0u8; 2];
        r.read_exact(&mut len).await.unwrap();
        let n = u16::from_be_bytes(len) as usize;
        let mut buf = vec![0u8; n];
        r.read_exact(&mut buf).await.unwrap();
        if corrupt && !buf.is_empty() {
            buf[0] ^= 0xff;
        }
        w.write_all(&len).await.unwrap();
        w.write_all(&buf).await.unwrap();
        w.flush().await.unwrap();
    }
}
