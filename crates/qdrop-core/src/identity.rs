//! Long-term device identity: a persistent Ed25519 key pair and the
//! self-signed certificate wrapped around it for TLS.
//!
//! The certificate is disposable — it is regenerated from the key on every
//! start. Trust is pinned to the raw Ed25519 public key (see [`crate::tls`]),
//! never to the cert bytes or a CA.

use std::path::Path;

use anyhow::{Context, Result};
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::crypto::sha256;

/// DNS name embedded in every qdrop certificate. The pinning verifier ignores
/// it, but rustls requires *a* name on the client side.
pub const CERT_DNS_NAME: &str = "qdrop.local";

/// A loaded identity: key pair plus a fresh cert/key pair in rustls form.
#[derive(Debug)]
pub struct Identity {
    key_pair: KeyPair,
    cert_der: CertificateDer<'static>,
    key_der: PrivateKeyDer<'static>,
}

impl Identity {
    /// Load the key from `~/.config/qdrop/identity.pem`, creating it on first
    /// run, then build a self-signed certificate around it.
    pub fn load_or_create() -> Result<Self> {
        Self::load_or_create_at(crate::paths::identity_file()?)
    }

    /// Testable variant at an explicit path.
    pub fn load_or_create_at(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let key_pair = match std::fs::read_to_string(path) {
            Ok(pem) => KeyPair::from_pem(&pem)
                .with_context(|| format!("parsing identity key {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let kp = KeyPair::generate_for(&PKCS_ED25519).context("generating Ed25519 key")?;
                crate::paths::write_atomic(path, kp.serialize_pem().as_bytes())?;
                kp
            }
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", path.display()));
            }
        };
        Self::from_key_pair(key_pair)
    }

    fn from_key_pair(key_pair: KeyPair) -> Result<Self> {
        let params = CertificateParams::new(vec![CERT_DNS_NAME.to_string()])
            .context("building certificate params")?;
        let cert = params
            .self_signed(&key_pair)
            .context("self-signing certificate")?;
        let cert_der = cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
        Ok(Self {
            key_pair,
            cert_der,
            key_der,
        })
    }

    /// The raw 32-byte Ed25519 public key — this is the pinned identity.
    pub fn public_key(&self) -> [u8; 32] {
        let raw = self.key_pair.public_key_raw();
        let mut out = [0u8; 32];
        out.copy_from_slice(raw);
        out
    }

    /// Short human-facing fingerprint: first 8 bytes of SHA-256(pubkey), hex,
    /// grouped (e.g. `a1b2-c3d4-e5f6-0718`).
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.public_key())
    }

    /// Certificate chain for a rustls config (single self-signed cert).
    pub fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert_der.clone()]
    }

    /// Private key for a rustls config.
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }
}

/// Fingerprint format shared with `qdrop peers` output and pairing prompts.
pub fn fingerprint_of(public_key: &[u8; 32]) -> String {
    let d = sha256(public_key);
    format!(
        "{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}",
        d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_key_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.pem");

        let a = Identity::load_or_create_at(&path).unwrap();
        let b = Identity::load_or_create_at(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(a.fingerprint(), b.fingerprint());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn cert_embeds_the_identity_key() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create_at(dir.path().join("id.pem")).unwrap();
        let chain = id.cert_chain();
        let key = crate::tls::peer_key_from_cert(&chain[0]).unwrap();
        assert_eq!(key, id.public_key());
    }
}
