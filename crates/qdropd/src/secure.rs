//! TLS wrapping for the steady-state transport (M2).
//!
//! The dialer is the TLS client and pins the specific server key it expects;
//! the acceptor is the TLS server and accepts any key currently in the
//! roster, learning *which* peer connected from the client certificate.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use qdrop_core::identity::Identity;
use qdrop_core::roster::Roster;
use qdrop_core::tls;
use rustls::ServerConfig;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

/// An encrypted, mutually-authenticated stream.
pub type SecureStream = TlsStream<TcpStream>;

/// Dial side: connect TLS to a server whose key must equal `expect`.
pub async fn connect(
    tcp: TcpStream,
    identity: &Identity,
    expect: [u8; 32],
    limit: Duration,
) -> Result<SecureStream> {
    let config = tls::client_config(identity, expect)?;
    let connector = TlsConnector::from(config);
    let tls = timeout(limit, connector.connect(tls::server_name(), tcp))
        .await
        .context("TLS handshake timed out")?
        .context("TLS handshake failed")?;
    Ok(SecureStream::from(tls))
}

/// Accept side: complete the TLS handshake and return the peer's pinned key.
pub async fn accept(
    tcp: TcpStream,
    server_config: Arc<ServerConfig>,
    roster: &Roster,
    limit: Duration,
) -> Result<(SecureStream, [u8; 32])> {
    let acceptor = TlsAcceptor::from(server_config);
    let tls = timeout(limit, acceptor.accept(tcp))
        .await
        .context("TLS handshake timed out")?
        .context("TLS handshake failed")?;

    let key = {
        let (_, state) = tls.get_ref();
        let cert = state
            .peer_certificates()
            .and_then(|c| c.first())
            .context("peer presented no certificate")?;
        tls::peer_key_from_cert(cert)?
    };

    // The verifier already accepted it, but confirm it is still trusted (the
    // roster may have changed between the handshake and now).
    if !roster.read().is_trusted_key(&key) {
        anyhow::bail!("peer key is no longer in the roster");
    }
    Ok((SecureStream::from(tls), key))
}
