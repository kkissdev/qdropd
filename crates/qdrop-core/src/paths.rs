//! Filesystem locations qdrop uses.
//!
//! The design pins config to `~/.config/qdrop/` on *both* macOS and Linux
//! (rather than `~/Library/Application Support` on macOS), so we resolve it
//! by hand from `XDG_CONFIG_HOME` / `HOME`. `QDROP_CONFIG_DIR` overrides
//! everything, which keeps tests hermetic.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Directory holding `config.toml` and `peers.toml`.
pub fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("QDROP_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(xdg).join("qdrop"));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .context("neither QDROP_CONFIG_DIR, XDG_CONFIG_HOME, nor HOME is set")?;
    Ok(PathBuf::from(home).join(".config").join("qdrop"))
}

/// Path to the main config file.
pub fn config_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Path to the paired-peer registry.
pub fn peers_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("peers.toml"))
}

/// Path to the long-term identity key (PKCS#8 PEM, mode 0600).
pub fn identity_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("identity.pem"))
}

/// Path to the daemon's live-status file (JSON), read by `qdrop peers`.
pub fn state_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("state.json"))
}

/// Write `contents` to `path` atomically (temp file in the same directory,
/// then rename). Creates parent directories. On unix the file is 0600.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let parent = path.parent().context("path has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;

    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("qdrop"),
        std::process::id()
    ));
    let mut f =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(contents)
        .and_then(|_| f.sync_all())
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))
}

/// Directory incoming files land in (`qdrop send`, M4+).
pub fn downloads_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("QDROP_DOWNLOAD_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .context("HOME is not set")?;
    Ok(PathBuf::from(home).join("Downloads").join("qdrop"))
}
