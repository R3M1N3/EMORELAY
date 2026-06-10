//! 隧道线协议(P3b)。
//! - 连接级 1 字节 stream preamble:entry dial 后写,exit accept 后读;mid 不感知。
//!   0x01 = TCP 业务流(后续为裸字节);0x02 = UDP 帧流(后续为帧序列)。
//! - UDP 帧:2 字节大端长度前缀 + payload(spec §4.6)。单包 ≤65507 天然放进 u16。
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const STREAM_TCP: u8 = 0x01;
pub const STREAM_UDP: u8 = 0x02;

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = u16::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "udp payload > 65535")
    })?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    // WSS 等带缓冲 transport 需要 flush 把整帧推成一条消息。
    w.flush().await
}

/// 读一帧进 buf(覆盖式 resize),返回 payload 长度。EOF/对端关闭 → Err(UnexpectedEof)。
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut len_bytes = [0u8; 2];
    r.read_exact(&mut len_bytes).await?;
    let len = u16::from_be_bytes(len_bytes) as usize;
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip_including_empty() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, b"hello").await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        let mut buf = Vec::new();
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn oversized_payload_rejected() {
        let (mut a, _b) = tokio::io::duplex(1024);
        let big = vec![0u8; 65536];
        assert!(write_frame(&mut a, &big).await.is_err());
    }
}
