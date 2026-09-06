//! Length-prefixed msgpack framing.
//!
//! Wire format: a 4-byte big-endian unsigned length, then that many bytes of
//! `rmp-serde` (named-field) msgpack encoding a [`Message`].

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::proto::Message;

/// Hard cap on a single frame's payload. Blob chunks (M4) are ~64 KiB, so
/// 16 MiB is generous; anything larger is treated as a protocol error rather
/// than allocated.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Errors reading a frame.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("connection closed")]
    Closed,
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(usize),
    #[error("malformed frame: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl FrameError {
    /// Whether this looks like an orderly peer disconnect rather than a bug.
    pub fn is_disconnect(&self) -> bool {
        match self {
            FrameError::Closed => true,
            FrameError::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ),
            _ => false,
        }
    }
}

/// Write one message. Flushes before returning.
pub async fn write_message<W>(w: &mut W, msg: &Message) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = rmp_serde::to_vec_named(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    debug_assert!(body.len() <= MAX_FRAME_LEN);
    let len = u32::try_from(body.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Read one message. `Err(FrameError::Closed)` means a clean EOF at a frame
/// boundary (the peer hung up between frames).
pub async fn read_message<R>(r: &mut R) -> Result<Message, FrameError>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(rmp_serde::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Caps, Hello, PROTOCOL_VERSION};

    #[tokio::test]
    async fn roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let sent = Message::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: "id".into(),
            device_name: "name".into(),
            caps: Caps::ALL,
        });
        let s2 = sent.clone();
        let writer = tokio::spawn(async move { write_message(&mut a, &s2).await });

        let got = read_message(&mut b).await.unwrap();
        assert_eq!(got, sent);
        writer.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn multiple_frames_stay_aligned() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            for seq in 0..5 {
                write_message(&mut a, &Message::Ping { seq }).await.unwrap();
            }
        });
        for seq in 0..5 {
            assert_eq!(read_message(&mut b).await.unwrap(), Message::Ping { seq });
        }
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn clean_eof_reports_closed() {
        let (a, mut b) = tokio::io::duplex(1024);
        drop(a);
        assert!(matches!(
            read_message(&mut b).await,
            Err(FrameError::Closed)
        ));
    }

    #[tokio::test]
    async fn arbitrary_bytes_never_panic() {
        // A cheap stand-in for the fuzz target: many pseudo-random inputs must
        // all resolve to Ok/Err without panicking.
        let mut seed = 0x9e3779b97f4a7c15u64;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (seed >> 24) as usize % 512;
            let bytes: Vec<u8> = (0..len)
                .map(|i| {
                    let x = seed.wrapping_add(i as u64).wrapping_mul(0xff51afd7ed558ccd);
                    (x >> 33) as u8
                })
                .collect();
            let mut cur = std::io::Cursor::new(bytes);
            for _ in 0..8 {
                if read_message(&mut cur).await.is_err() {
                    break;
                }
            }
        }
    }

    #[tokio::test]
    async fn oversize_length_is_rejected() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            let _ = a.write_all(&u32::MAX.to_be_bytes()).await;
            // keep the writer alive so the reader doesn't see EOF first
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
        assert!(matches!(
            read_message(&mut b).await,
            Err(FrameError::TooLarge(_))
        ));
    }
}
