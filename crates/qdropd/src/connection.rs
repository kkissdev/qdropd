//! A single live peer connection: `Hello` handshake, then `Ping`/`Pong`
//! keepalive until the link drops.
//!
//! Plaintext for M1 — `rustls` wraps this in M2. Nothing here assumes TCP,
//! so the TLS swap is just a different stream type.

use std::time::Duration;

use qdrop_core::frame::{read_message, write_message};
#[cfg(test)]
use qdrop_core::proto::ClipEntry;
use qdrop_core::proto::{Caps, Hello, Message, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, timeout, Instant, MissedTickBehavior};

/// Keepalive tuning.
#[derive(Debug, Clone, Copy)]
pub struct KeepAlive {
    /// How often to send a `Ping`.
    pub interval: Duration,
    /// Drop the connection if no frame arrives within this window.
    pub idle_timeout: Duration,
    /// Cap on how long the `Hello` exchange may take.
    pub handshake_timeout: Duration,
}

impl Default for KeepAlive {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(45),
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

/// Outcome of a completed connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    PeerClosed,
    IdleTimeout,
    WriteError,
    ProtocolError(String),
}

impl std::fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DisconnectReason::PeerClosed => write!(f, "peer closed the connection"),
            DisconnectReason::IdleTimeout => write!(f, "idle timeout"),
            DisconnectReason::WriteError => write!(f, "write error"),
            DisconnectReason::ProtocolError(e) => write!(f, "protocol error: {e}"),
        }
    }
}

/// What we learned during the handshake.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub peer: Hello,
    /// Capabilities both sides support.
    pub effective_caps: Caps,
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("handshake timed out")]
    Timeout,
    #[error("peer speaks protocol v{their}, we speak v{ours}")]
    VersionMismatch { ours: u32, their: u32 },
    #[error("peer reported a different device id than mDNS advertised")]
    IdentityMismatch,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("malformed handshake: {0}")]
    Frame(String),
}

/// Exchange `Hello`s. Both sides send immediately then read, which cannot
/// deadlock for frames this small. `expect_id`, when set, must match the id
/// the peer reports (guards the dialer against connecting to the wrong box).
pub async fn handshake<S>(
    stream: &mut S,
    local: &Hello,
    expect_id: Option<&str>,
    limit: Duration,
) -> Result<Handshake, HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(limit, async {
        write_message(stream, &Message::Hello(local.clone())).await?;
        let peer = match read_message(stream).await {
            Ok(Message::Hello(h)) => h,
            Ok(other) => {
                return Err(HandshakeError::Frame(format!(
                    "expected Hello, got {other:?}"
                )))
            }
            Err(e) => return Err(HandshakeError::Frame(e.to_string())),
        };

        if peer.protocol_version != PROTOCOL_VERSION {
            return Err(HandshakeError::VersionMismatch {
                ours: PROTOCOL_VERSION,
                their: peer.protocol_version,
            });
        }
        if let Some(id) = expect_id {
            if peer.device_id != id {
                return Err(HandshakeError::IdentityMismatch);
            }
        }

        let effective_caps = local.caps.intersect(peer.caps);
        Ok(Handshake {
            peer,
            effective_caps,
        })
    })
    .await
    .map_err(|_| HandshakeError::Timeout)?
}

/// Application-facing wiring for a live connection: non-keepalive frames from
/// the peer go out on `inbound`, and anything pushed to `outbound` is written
/// to the peer.
pub struct Wire {
    pub peer_id: String,
    pub inbound: mpsc::Sender<(String, Message)>,
    pub outbound: mpsc::Receiver<Message>,
}

/// Run the keepalive + application loop until the connection ends. Assumes the
/// handshake has already completed on `stream`.
///
/// Frame reads happen in a dedicated task: `read_message` is not
/// cancellation-safe (it can consume a length prefix without its body), so it
/// must never sit in a `select!` arm. The task forwards decoded frames over a
/// channel, and `recv` *is* cancel-safe.
pub async fn run<S>(stream: S, ka: KeepAlive, mut wire: Wire) -> DisconnectReason
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (frames_tx, mut frames_rx) = mpsc::channel::<Result<Message, String>>(16);
    let reader = tokio::spawn(async move {
        loop {
            match read_message(&mut rd).await {
                Ok(msg) => {
                    if frames_tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
                Err(e) if e.is_disconnect() => {
                    let _ = frames_tx.send(Err(String::new())).await;
                    break;
                }
                Err(e) => {
                    let _ = frames_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    let mut ping = interval(ka.interval);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await; // the first tick fires immediately; skip it
    let mut seq: u64 = 0;
    // Idle is measured from the last frame *received*, not the last loop turn,
    // so our own outgoing pings never postpone the timeout.
    let mut idle_deadline = Instant::now() + ka.idle_timeout;

    let reason = loop {
        tokio::select! {
            biased;
            _ = sleep_until(idle_deadline) => break DisconnectReason::IdleTimeout,
            _ = ping.tick() => {
                seq += 1;
                if write_message(&mut wr, &Message::Ping { seq }).await.is_err() {
                    break DisconnectReason::WriteError;
                }
            }
            out = wire.outbound.recv() => {
                match out {
                    Some(msg) => {
                        if write_message(&mut wr, &msg).await.is_err() {
                            break DisconnectReason::WriteError;
                        }
                    }
                    None => break DisconnectReason::PeerClosed, // app side gone
                }
            }
            frame = frames_rx.recv() => {
                match frame {
                    None => break DisconnectReason::PeerClosed,
                    Some(Err(e)) if e.is_empty() => break DisconnectReason::PeerClosed,
                    Some(Err(e)) => break DisconnectReason::ProtocolError(e),
                    Some(Ok(msg)) => {
                        idle_deadline = Instant::now() + ka.idle_timeout;
                        match msg {
                            Message::Ping { seq } => {
                                if write_message(&mut wr, &Message::Pong { seq }).await.is_err() {
                                    break DisconnectReason::WriteError;
                                }
                            }
                            Message::Pong { .. } => {}
                            Message::Hello(_) => {
                                tracing::warn!("unexpected Hello mid-connection, ignoring");
                            }
                            other => {
                                // Application frame — hand it to the subsystem.
                                if wire.inbound.send((wire.peer_id.clone(), other)).await.is_err() {
                                    break DisconnectReason::PeerClosed;
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    reader.abort();
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(id: &str) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: id.into(),
            device_name: id.into(),
            caps: Caps::ALL,
        }
    }

    #[tokio::test]
    async fn handshake_pairs_up_and_negotiates_caps() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let mut a_local = hello("a");
        a_local.caps = Caps {
            open_url: false,
            ..Caps::ALL
        };
        let b_local = hello("b");

        let ha = tokio::spawn(async move {
            handshake(&mut a, &a_local, Some("b"), Duration::from_secs(2)).await
        });
        let hb = handshake(&mut b, &b_local, Some("a"), Duration::from_secs(2))
            .await
            .unwrap();

        let ha = ha.await.unwrap().unwrap();
        assert_eq!(ha.peer.device_id, "b");
        assert_eq!(hb.peer.device_id, "a");
        // 'a' does not offer open_url, so the effective set drops it on both ends.
        assert!(!ha.effective_caps.open_url);
        assert!(!hb.effective_caps.open_url);
        assert!(ha.effective_caps.clipboard_text);
    }

    #[tokio::test]
    async fn handshake_rejects_wrong_identity() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let a_local = hello("a");
        let b_local = hello("b");
        let ha =
            tokio::spawn(
                async move { handshake(&mut a, &a_local, None, Duration::from_secs(2)).await },
            );
        let err = handshake(&mut b, &b_local, Some("not-a"), Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, HandshakeError::IdentityMismatch));
        let _ = ha.await;
    }

    fn test_wire() -> (
        Wire,
        mpsc::Receiver<(String, Message)>,
        mpsc::Sender<Message>,
    ) {
        let (in_tx, in_rx) = mpsc::channel(16);
        let (out_tx, out_rx) = mpsc::channel(16);
        (
            Wire {
                peer_id: "peer".into(),
                inbound: in_tx,
                outbound: out_rx,
            },
            in_rx,
            out_tx,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn run_exchanges_keepalive_then_notices_idle() {
        let (client, server) = tokio::io::duplex(8192);
        let ka = KeepAlive {
            interval: Duration::from_secs(1),
            idle_timeout: Duration::from_secs(3),
            handshake_timeout: Duration::from_secs(1),
        };

        // Server side answers pings for a while, then goes silent.
        let server = tokio::spawn(async move {
            let (mut rd, mut wr) = tokio::io::split(server);
            for _ in 0..3 {
                match read_message(&mut rd).await.unwrap() {
                    Message::Ping { seq } => write_message(&mut wr, &Message::Pong { seq })
                        .await
                        .unwrap(),
                    _ => panic!("expected ping"),
                }
            }
            // stop responding; keep the socket open
            std::future::pending::<()>().await;
        });

        let (wire, _in_rx, _out_tx) = test_wire();
        let reason = run(client, ka, wire).await;
        assert_eq!(reason, DisconnectReason::IdleTimeout);
        server.abort();
    }

    #[tokio::test]
    async fn run_reports_peer_closed() {
        let (client, server) = tokio::io::duplex(8192);
        drop(server);
        let (wire, _in_rx, _out_tx) = test_wire();
        assert_eq!(
            run(client, KeepAlive::default(), wire).await,
            DisconnectReason::PeerClosed
        );
    }

    #[tokio::test]
    async fn run_relays_application_frames_both_ways() {
        let (client, server) = tokio::io::duplex(8192);
        let (wire, mut in_rx, out_tx) = test_wire();
        let h = tokio::spawn(run(client, KeepAlive::default(), wire));

        let (mut srd, mut swr) = tokio::io::split(server);
        // peer -> us
        write_message(
            &mut swr,
            &Message::Clipboard {
                seq: 1,
                origin_id: "x".into(),
                entries: vec![ClipEntry::text("hi")],
            },
        )
        .await
        .unwrap();
        let (pid, msg) = in_rx.recv().await.unwrap();
        assert_eq!(pid, "peer");
        assert!(matches!(msg, Message::Clipboard { seq: 1, .. }));

        // us -> peer
        out_tx
            .send(Message::Clipboard {
                seq: 2,
                origin_id: "y".into(),
                entries: vec![],
            })
            .await
            .unwrap();
        match read_message(&mut srd).await.unwrap() {
            Message::Clipboard { seq, .. } => assert_eq!(seq, 2),
            other => panic!("got {other:?}"),
        }
        drop(out_tx);
        drop(in_rx);
        h.abort();
    }
}
