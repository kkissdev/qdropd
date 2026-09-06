//! Mutually-authenticated TLS 1.3 with **pinned** Ed25519 keys.
//!
//! No CA, no TOFU, no name checking: a peer is trusted iff the raw public key
//! in its certificate is one we recorded during pairing. The handshake
//! signature is still verified (via the ring provider) so presenting someone
//! else's certificate does not help an attacker.

use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};

use crate::crypto::ct_eq;
use crate::identity::{Identity, CERT_DNS_NAME};
use crate::roster::Roster;

fn provider_algs() -> WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

/// The server name to hand `TlsConnector::connect` (the verifier ignores it).
pub fn server_name() -> ServerName<'static> {
    ServerName::try_from(CERT_DNS_NAME).expect("static name is valid")
}

/// Extract the raw Ed25519 public key from a DER certificate.
pub fn peer_key_from_cert(cert: &CertificateDer<'_>) -> Result<[u8; 32]> {
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|e| anyhow::anyhow!("parsing peer certificate: {e}"))?;
    let spki = parsed.public_key().subject_public_key.data.as_ref();
    if spki.len() != 32 {
        anyhow::bail!(
            "peer key is {} bytes, expected a 32-byte Ed25519 key",
            spki.len()
        );
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(spki);
    Ok(out)
}

/// Client config that will only complete a handshake with the server whose
/// key is `expect`.
pub fn client_config(identity: &Identity, expect: [u8; 32]) -> Result<Arc<ClientConfig>> {
    let verifier = Arc::new(PinnedServer {
        expect,
        algs: provider_algs(),
    });
    let cfg =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("selecting TLS 1.3")?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(identity.cert_chain(), identity.private_key())
            .context("installing client identity")?;
    Ok(Arc::new(cfg))
}

/// Server config that requires client auth and accepts any key in `roster`.
pub fn server_config(identity: &Identity, roster: Roster) -> Result<Arc<ServerConfig>> {
    let verifier = Arc::new(PinnedClient {
        roster,
        algs: provider_algs(),
    });
    let cfg =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .context("selecting TLS 1.3")?
            .with_client_cert_verifier(verifier)
            .with_single_cert(identity.cert_chain(), identity.private_key())
            .context("installing server identity")?;
    Ok(Arc::new(cfg))
}

// --- verifiers ---------------------------------------------------------------

#[derive(Debug)]
struct PinnedServer {
    expect: [u8; 32],
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match peer_key_from_cert(end_entity) {
            Ok(key) if ct_eq(&key, &self.expect) => Ok(ServerCertVerified::assertion()),
            Ok(_) => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            )),
            Err(_) => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedClient {
    roster: Roster,
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PinnedClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let key = peer_key_from_cert(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        if self.roster.read().is_trusted_key(&key) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    async fn identity(dir: &std::path::Path, name: &str) -> Identity {
        Identity::load_or_create_at(dir.join(name)).unwrap()
    }

    fn roster_with(peers: &[(&str, &Identity)]) -> crate::roster::Roster {
        let mut doc = crate::Peers::default();
        for (name, id) in peers {
            doc.upsert(crate::Peer::new(
                *name,
                format!("id-{name}"),
                &id.public_key(),
            ));
        }
        crate::roster::Roster::from_peers(&doc)
    }

    // A pinned client and server complete a handshake; a third identity the
    // server has not pinned is rejected.
    #[tokio::test]
    async fn pinned_peers_connect_and_stranger_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let server_id = identity(dir.path(), "server.pem").await;
        let client_id = identity(dir.path(), "client.pem").await;
        let stranger_id = identity(dir.path(), "stranger.pem").await;

        let ros = roster_with(&[("client", &client_id)]);

        let acceptor = TlsAcceptor::from(server_config(&server_id, ros.clone()).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_key = server_id.public_key();

        let srv = tokio::spawn(async move {
            let mut ok = 0u32;
            for _ in 0..2 {
                let (sock, _) = listener.accept().await.unwrap();
                if let Ok(mut tls) = acceptor.accept(sock).await {
                    let mut b = [0u8; 4];
                    if tls.read_exact(&mut b).await.is_ok() {
                        ok += 1;
                        let _ = tls.write_all(b"pong").await;
                    }
                }
            }
            ok
        });

        // Pinned client: succeeds.
        {
            let conn = TlsConnector::from(client_config(&client_id, server_key).unwrap());
            let sock = TcpStream::connect(addr).await.unwrap();
            let mut tls = conn.connect(server_name(), sock).await.unwrap();
            tls.write_all(b"ping").await.unwrap();
            let mut b = [0u8; 4];
            tls.read_exact(&mut b).await.unwrap();
            assert_eq!(&b, b"pong");
        }

        // Unpinned stranger: the server refuses client auth. In TLS 1.3 the
        // client finishes its flight before the server's alert arrives, so the
        // rejection may surface as a failed connect *or* as a failed first
        // read — either way, no data round-trips.
        {
            let conn = TlsConnector::from(client_config(&stranger_id, server_key).unwrap());
            let sock = TcpStream::connect(addr).await.unwrap();
            let refused = match conn.connect(server_name(), sock).await {
                Err(_) => true,
                Ok(mut tls) => {
                    let _ = tls.write_all(b"ping").await;
                    let _ = tls.flush().await;
                    let mut b = [0u8; 4];
                    tls.read_exact(&mut b).await.is_err()
                }
            };
            assert!(refused, "server must reject an unpinned client");
        }

        assert_eq!(srv.await.unwrap(), 1);
    }

    // Client pins the wrong server key -> client aborts.
    #[tokio::test]
    async fn client_rejects_wrong_server_key() {
        let dir = tempfile::tempdir().unwrap();
        let server_id = identity(dir.path(), "s.pem").await;
        let client_id = identity(dir.path(), "c.pem").await;
        let other_id = identity(dir.path(), "o.pem").await;

        let ros = roster_with(&[("c", &client_id)]);
        let acceptor = TlsAcceptor::from(server_config(&server_id, ros).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((sock, _)) = listener.accept().await {
                let _ = acceptor.accept(sock).await;
            }
        });

        let conn = TlsConnector::from(client_config(&client_id, other_id.public_key()).unwrap());
        let sock = TcpStream::connect(addr).await.unwrap();
        assert!(conn.connect(server_name(), sock).await.is_err());
    }
}
