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
