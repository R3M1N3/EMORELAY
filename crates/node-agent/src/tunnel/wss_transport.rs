//! WSS transport(P3b):WebSocket over TLS。TLS 配置/SNI 复用 TlsTransport,
//! tungstenite 只做 ws 协议层。WsByteStream 把 Binary message 流适配成
//! AsyncRead/AsyncWrite:write → 一条 Binary;read → 按序消费 Binary 载荷;
//! Ping/Pong 由 tungstenite 自动应答,Text 忽略,Close/流终止 → EOF。
//!
//! **限制:WebSocket 没有 TCP 半关(half-close)**。poll_shutdown 发 Close frame
//! 终结整条连接(保证资源释放),一端 shutdown 写半后对端尚未发出的反向数据可能
//! 被截断。依赖半关终止的业务流(HTTP/1.0 靠 FIN 界定 body 等)请选 tcp/tls。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelContext;
use futures_util::{Sink, Stream};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async, client_async, WebSocketStream};

use crate::tunnel::tls_transport::TlsTransport;
use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct WssTransport {
    tls: TlsTransport,
    /// client_async 只用 URL 写 Host/路径,TLS 已在下层完成,故 scheme 用 ws://。
    dial_url: String,
}

impl WssTransport {
    pub fn load(data_dir: &str, ctx: &TunnelContext) -> Result<Self> {
        let tls = TlsTransport::load(data_dir, ctx)?;
        let sni = format!(
            "tunnel-{}-hop-{}.emorelay.internal",
            ctx.tunnel_id,
            ctx.self_ordinal + 1
        );
        Ok(Self { tls, dial_url: format!("ws://{sni}/tunnel") })
    }
}

#[tonic::async_trait]
impl TunnelTransport for WssTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let tcp = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel wss tcp connect {addr}"))?;
        let tls = self
            .tls
            .connector
            .connect(self.tls.dial_sni.clone(), tcp)
            .await
            .context("tunnel wss tls handshake")?;
        let (ws, _resp) = client_async(self.dial_url.as_str(), tls)
            .await
            .context("tunnel ws client handshake")?;
        Ok(Box::new(WsByteStream::new(ws)))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel wss bind {addr}"))?;
        Ok(Box::new(WssTunnelListener {
            inner: l,
            acceptor: self.tls.acceptor.clone(),
        }))
    }
}

struct WssTunnelListener {
    inner: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

#[tonic::async_trait]
impl TunnelListener for WssTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (tcp, _) = self.inner.accept().await.context("tunnel wss tcp accept")?;
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .context("tunnel wss tls handshake")?;
        let ws = accept_async(tls).await.context("tunnel ws server handshake")?;
        Ok(Box::new(WsByteStream::new(ws)))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}

// ============= WsByteStream =============

fn to_io(e: tokio_tungstenite::tungstenite::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e)
}

struct WsByteStream<S> {
    inner: WebSocketStream<S>,
    read_buf: Vec<u8>,
    read_pos: usize,
}

impl<S> WsByteStream<S> {
    fn new(inner: WebSocketStream<S>) -> Self {
        Self { inner, read_buf: Vec::new(), read_pos: 0 }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> AsyncRead for WsByteStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.read_pos < self.read_buf.len() {
                let n = (self.read_buf.len() - self.read_pos).min(buf.remaining());
                let pos = self.read_pos;
                buf.put_slice(&self.read_buf[pos..pos + n]);
                self.read_pos += n;
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                // 流终止 / 对端 Close → EOF(空读)。
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(Message::Binary(data)))) => {
                    self.read_buf = data.into();
                    self.read_pos = 0;
                }
                Poll::Ready(Some(Ok(Message::Close(_)))) => return Poll::Ready(Ok(())),
                // Ping/Pong 由 tungstenite 自动应答;Text/Frame 对字节流无意义,跳过。
                Poll::Ready(Some(Ok(_))) => continue,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(to_io(e))),
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> AsyncWrite for WsByteStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_ready(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(to_io(e))),
            Poll::Ready(Ok(())) => {
                Pin::new(&mut self.inner)
                    .start_send(Message::Binary(data.to_vec().into()))
                    .map_err(to_io)?;
                Poll::Ready(Ok(data.len()))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx).map_err(to_io)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx).map_err(to_io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::testutil::write_hop_creds_pair;
    use crate::tunnel::transport::TunnelTransport;
    use emorelay_common::control::v1::TunnelContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ctx(ordinal: u32) -> TunnelContext {
        TunnelContext {
            tunnel_id: 9,
            role: 0,
            next_hop_addr: String::new(),
            next_hop_inter_port: 0,
            self_inter_port: 0,
            transport: "wss".into(),
            self_ordinal: ordinal,
        }
    }

    #[tokio::test]
    async fn wss_transport_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;

        let server_t = WssTransport::load(&data_dir, &ctx(1)).expect("server load");
        let client_t = WssTransport::load(&data_dir, &ctx(0)).expect("client load");

        let mut listener = server_t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("wss accept");
            let mut buf = [0u8; 5];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"hello");
            conn.write_all(b"world").await.unwrap();
            // 显式 flush:WsByteStream 写入是消息缓冲语义。
            conn.flush().await.unwrap();
        });

        let mut conn = client_t.dial(&addr.to_string()).await.expect("wss dial");
        conn.write_all(b"hello").await.unwrap();
        conn.flush().await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"world");
        server.await.unwrap();
    }

    /// 16 字节消息用 4 字节缓冲读四次,覆盖 WsByteStream read_pos 跨 poll 部分消费路径。
    #[tokio::test]
    async fn ws_byte_stream_partial_read_drains_across_polls() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;

        let server_t = WssTransport::load(&data_dir, &ctx(1)).expect("server load");
        let client_t = WssTransport::load(&data_dir, &ctx(0)).expect("client load");

        let mut listener = server_t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();

        // 服务端:写一条 16 字节消息后关闭连接。
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("wss accept");
            conn.write_all(&[1u8; 16]).await.unwrap();
            conn.flush().await.unwrap();
            conn.shutdown().await.unwrap();
        });

        // 客户端:用 4 字节缓冲分四次 read_exact,验证 partial-drain 路径正确拼接。
        let mut conn = client_t.dial(&addr.to_string()).await.expect("wss dial");
        let mut collected = Vec::new();
        for _ in 0..4 {
            let mut chunk = [0u8; 4];
            conn.read_exact(&mut chunk).await.unwrap();
            collected.extend_from_slice(&chunk);
        }
        assert_eq!(collected, vec![1u8; 16]);

        server.await.unwrap();
    }

    /// 大 payload(单次 write_all → 单条大 Binary 消息)完整往返;
    /// tungstenite 默认 max_message_size(64MB)远大于 256KB。
    #[tokio::test]
    async fn wss_transport_large_payload() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let server_t = WssTransport::load(&data_dir, &ctx(1)).unwrap();
        let client_t = WssTransport::load(&data_dir, &ctx(0)).unwrap();

        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = vec![0xCD_u8; 256 * 1024];
        let expect = payload.clone();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            let mut buf = vec![0u8; expect.len()];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, expect);
        });

        let mut conn = client_t.dial(&addr.to_string()).await.unwrap();
        conn.write_all(&payload).await.unwrap();
        conn.flush().await.unwrap();
        // 半关写端让对端 read_exact 后不会悬挂在 EOF 判定上。
        conn.shutdown().await.unwrap();
        server.await.unwrap();
    }
}
