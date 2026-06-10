//! 裸 TCP transport(P3b)。无加密——仅适合内网/测试,生产推荐 tls/wss。
use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct TcpTransport;

#[tonic::async_trait]
impl TunnelTransport for TcpTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let s = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel tcp dial {addr}"))?;
        Ok(Box::new(s))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tcp bind {addr}"))?;
        Ok(Box::new(TcpTunnelListener { inner: l }))
    }
}

struct TcpTunnelListener {
    inner: TcpListener,
}

#[tonic::async_trait]
impl TunnelListener for TcpTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (s, _) = self.inner.accept().await.context("tunnel tcp accept")?;
        Ok(Box::new(s))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::transport::TunnelTransport;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_transport_dial_bind_roundtrip() {
        let t = TcpTransport;
        let mut listener = t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            conn.write_all(b"pong").await.unwrap();
        });

        let mut conn = t.dial(&addr.to_string()).await.expect("dial");
        conn.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn make_transport_tcp_ok_unknown_rejected() {
        use emorelay_common::control::v1::TunnelContext;
        let ctx = |transport: &str| TunnelContext {
            tunnel_id: 1,
            role: 1,
            next_hop_addr: "127.0.0.1".into(),
            next_hop_inter_port: 1,
            self_inter_port: 0,
            transport: transport.into(),
            self_ordinal: 0,
        };
        assert!(crate::tunnel::make_transport(&ctx("tcp"), "./x").is_ok());
        assert!(crate::tunnel::make_transport(&ctx("quic"), "./x").is_err());
    }
}
