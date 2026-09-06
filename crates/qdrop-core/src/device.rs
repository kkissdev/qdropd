//! Stable per-installation device identity.
//!
//! Until pairing (M2) introduces long-term Ed25519 keys, the daemon needs
//! *some* stable identifier to advertise over mDNS and to break the
//! dial/accept tie. This is a random 128-bit value, hex-encoded, persisted
//! next to the config.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Path to the persisted device id (`~/.config/qdrop/device_id`).
pub fn device_id_file() -> Result<PathBuf> {
    Ok(crate::paths::config_dir()?.join("device_id"))
}

/// Load the device id, creating and persisting one on first run.
pub fn load_or_create() -> Result<String> {
    load_or_create_at(device_id_file()?)
}

/// Testable variant: load-or-create at an explicit path.
pub fn load_or_create_at(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let id = text.trim().to_string();
            if is_valid(&id) {
                Ok(id)
            } else {
                anyhow::bail!("device id file {} is corrupt", path.display());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = generate();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(path, format!("{id}\n"))
                .with_context(|| format!("writing {}", path.display()))?;
            Ok(id)
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A fresh random device id (32 lowercase hex chars).
pub fn generate() -> String {
    let mut bytes = [0u8; 16];
    fastrand::fill(&mut bytes);
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn is_valid(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_well_formed() {
        let id = generate();
        assert!(is_valid(&id), "{id:?}");
    }

    #[test]
    fn persists_and_reloads_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("device_id");
        let first = load_or_create_at(&path).unwrap();
        let second = load_or_create_at(&path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device_id");
        std::fs::write(&path, "not-a-valid-id").unwrap();
        assert!(load_or_create_at(&path).is_err());
    }
}
