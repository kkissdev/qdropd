//! Small crypto helpers shared by pairing and TLS pinning.

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Fill `N` bytes from the OS CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).context("reading from the OS RNG")?;
    Ok(buf)
}

/// A fresh 6-digit pairing PIN (`"000000".."999999"`), uniformly distributed.
pub fn gen_pin() -> Result<String> {
    // Rejection-sample a u32 to avoid modulo bias.
    loop {
        let n = u32::from_le_bytes(random_bytes::<4>()?);
        if let Some(limit) = (u32::MAX / 1_000_000).checked_mul(1_000_000) {
            if n < limit {
                return Ok(format!("{:06}", n % 1_000_000));
            }
        }
    }
}

/// SHA-256 digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// HMAC-SHA-256.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Constant-time equality for fixed-size byte arrays.
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_six_digits() {
        for _ in 0..50 {
            let pin = gen_pin().unwrap();
            assert_eq!(pin.len(), 6);
            assert!(pin.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn hmac_detects_tamper() {
        let k = b"key";
        let a = hmac_sha256(k, b"message");
        let b = hmac_sha256(k, b"messagf");
        assert!(!ct_eq(&a, &b));
        assert!(ct_eq(&a, &hmac_sha256(k, b"message")));
    }
}
