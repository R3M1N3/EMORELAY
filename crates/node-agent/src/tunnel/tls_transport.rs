//! TLS transport(P3b)。隧道 TLS 与控制面 mTLS 复用同一内置 CA;凭据由
//! Command.tunnel_credentials 下发落盘(creds.rs)。dial 方强制 SNI =
//! tunnel-<id>-hop-<self_ordinal+1>.emorelay.internal——身份验证用 SNI/SAN,
//! next_hop_addr 只用于路由。server 端 WebPkiClientVerifier 强制 client cert
//! 链到同 CA。TLS 握手在 accept() 内串行完成:hop 仅被上一跳(信任域内)连入,
//! 恶意半开连接风险低,MVP 接受。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelContext;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::tunnel::creds::hop_dir;
use crate::tunnel::transport::{TunnelConn, TunnelListener, TunnelTransport};

pub struct TlsTransport {
    pub(crate) connector: TlsConnector,
    pub(crate) acceptor: TlsAcceptor,
    pub(crate) dial_sni: ServerName<'static>,
}

impl TlsTransport {
    pub fn load(data_dir: &str, ctx: &TunnelContext) -> Result<Self> {
        let dir = hop_dir(data_dir, ctx.tunnel_id, ctx.self_ordinal);

        let mut roots = RootCertStore::empty();
        for cert in load_certs(&dir.join("ca.pem"))? {
            roots.add(cert).context("add tunnel ca root")?;
        }
        let roots = Arc::new(roots);

        let client_cfg = ClientConfig::builder()
            .with_root_certificates(roots.clone())
            .with_client_auth_cert(
                load_certs(&dir.join("client.pem"))?,
                load_key(&dir.join("client.key"))?,
            )
            .context("build tunnel tls client config")?;

        let verifier = WebPkiClientVerifier::builder(roots)
            .build()
            .context("build tunnel client cert verifier")?;
        let server_cfg = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                load_certs(&dir.join("server.pem"))?,
                load_key(&dir.join("server.key"))?,
            )
            .context("build tunnel tls server config")?;

        let sni = format!(
            "tunnel-{}-hop-{}.emorelay.internal",
            ctx.tunnel_id,
            ctx.self_ordinal + 1
        );
        Ok(Self {
            connector: TlsConnector::from(Arc::new(client_cfg)),
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            dial_sni: ServerName::try_from(sni).context("invalid tunnel sni")?,
        })
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let pem = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    rustls_pemfile::certs(&mut pem.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("parse certs in {}", path.display()))
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let pem = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    rustls_pemfile::private_key(&mut pem.as_slice())
        .with_context(|| format!("parse key in {}", path.display()))?
        .with_context(|| format!("no private key in {}", path.display()))
}

#[tonic::async_trait]
impl TunnelTransport for TlsTransport {
    async fn dial(&self, addr: &str) -> Result<TunnelConn> {
        let tcp = TcpStream::connect(addr)
            .await
            .with_context(|| format!("tunnel tls tcp connect {addr}"))?;
        let tls = self
            .connector
            .connect(self.dial_sni.clone(), tcp)
            .await
            .context("tunnel tls client handshake")?;
        Ok(Box::new(tls))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tls bind {addr}"))?;
        Ok(Box::new(TlsTunnelListener {
            inner: l,
            acceptor: self.acceptor.clone(),
        }))
    }
}

struct TlsTunnelListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

#[tonic::async_trait]
impl TunnelListener for TlsTunnelListener {
    async fn accept(&mut self) -> Result<TunnelConn> {
        let (tcp, _) = self.inner.accept().await.context("tunnel tls tcp accept")?;
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .context("tunnel tls server handshake")?;
        Ok(Box::new(tls))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::testutil::write_hop_creds_pair;
    use crate::tunnel::transport::TunnelTransport;
    use emorelay_common::control::v1::TunnelContext;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ctx(tunnel_id: i64, ordinal: u32) -> TunnelContext {
        TunnelContext {
            tunnel_id,
            role: 0,
            next_hop_addr: String::new(),
            next_hop_inter_port: 0,
            self_inter_port: 0,
            transport: "tls".into(),
            self_ordinal: ordinal,
        }
    }

    /// hop-0(dial,SNI=hop-1) ↔ hop-1(accept,server SAN=hop-1):双向 mTLS 通,字节往返。
    #[tokio::test]
    async fn tls_transport_roundtrip_with_mutual_auth() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;

        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).expect("server load");
        let client_t = TlsTransport::load(&data_dir, &ctx(9, 0)).expect("client load");

        let mut listener = server_t.bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut conn = listener.accept().await.expect("tls accept");
            let mut buf = [0u8; 6];
            conn.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"secret");
            conn.write_all(b"shhh").await.unwrap();
        });

        let mut conn = client_t.dial(&addr.to_string()).await.expect("tls dial");
        conn.write_all(b"secret").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"shhh");
        server.await.unwrap();
    }

    /// 非 TLS 裸连必须被拒(握手失败即 accept Err;「合法 TLS 但无 client cert」
    /// 场景由 rustls WebPkiClientVerifier 强制,P3c e2e 真链路再覆盖)。
    #[tokio::test]
    async fn tls_transport_rejects_non_tls_client() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).unwrap();
        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // 裸 TCP 连入后立刻关闭(不做 TLS):accept 端握手必失败。
        let client = tokio::spawn(async move {
            let _ = tokio::net::TcpStream::connect(addr).await.unwrap();
        });
        assert!(listener.accept().await.is_err(), "无 TLS/无 client cert 必须被拒");
        let _ = client.await;
    }
}
