//! Client side of the daemon's Unix-socket control channel (newline-delimited
//! JSON). The daemon's server side lives in `qdropd`.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Path to the control socket. `QDROP_CONTROL_SOCK` wins (useful when the
/// config dir path would exceed the ~104-byte `sockaddr_un` limit), then
/// `$XDG_RUNTIME_DIR/qdropd.sock`, then `<config dir>/qdropd.sock`.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("QDROP_CONTROL_SOCK").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join("qdropd.sock"));
    }
    Ok(crate::paths::config_dir()?.join("qdropd.sock"))
}

/// Send one `{"cmd":"<cmd>"}` request and return the parsed JSON response.
/// Requires a tokio runtime.
pub async fn request(cmd: &str) -> Result<serde_json::Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path)
        .await
        .with_context(|| format!("connecting to {} (is qdropd running?)", path.display()))?;
    stream
        .write_all(format!("{{\"cmd\":\"{cmd}\"}}\n").as_bytes())
        .await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    serde_json::from_str(line.trim()).context("parsing control response")
}
