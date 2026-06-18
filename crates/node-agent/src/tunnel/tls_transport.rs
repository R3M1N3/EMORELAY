//! TLS transport(P3b)。隧道 TLS 与控制面 mTLS 复用同一内置 CA;凭据由
//! Command.tunnel_credentials 下发落盘(creds.rs)。dial 方强制 SNI =
//! tunnel-<id>-hop-<self_ordinal+1>.emorelay.internal——身份验证用 SNI/SAN,
//! next_hop_addr 只用于路由。server 端 WebPkiClientVerifier 强制 client cert
//! 链到同 CA。TLS 握手在 accept() 内串行完成(hop 仅被上一跳/信任域内连入);
//! 半开/慢握手连接由 HANDSHAKE_TIMEOUT(transport.rs)超时断开,防永久挂死 accept loop。
//!
//! P3c 补充:accept 后校验 client cert SAN = 上一跳(防同 CA 凭据横向连入)。
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
use crate::tunnel::transport::{
    PendingHop, TunnelConn, TunnelListener, TunnelTransport, HANDSHAKE_TIMEOUT,
};

pub struct TlsTransport {
    pub(crate) connector: TlsConnector,
    pub(crate) acceptor: TlsAcceptor,
    pub(crate) dial_sni: ServerName<'static>,
    /// server 端期望的 client cert SAN(= 上一跳)。entry(ordinal 0)无上一跳 → None,不允许 bind。
    pub(crate) expect_client_san: Option<String>,
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
        let expect_client_san = ctx.self_ordinal.checked_sub(1).map(|prev| {
            format!("tunnel-{}-hop-{}.emorelay.internal", ctx.tunnel_id, prev)
        });
        Ok(Self {
            connector: TlsConnector::from(Arc::new(client_cfg)),
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            dial_sni: ServerName::try_from(sni).context("invalid tunnel sni")?,
            expect_client_san,
        })
    }
}

/// 握手后校验 client cert 的 SAN 含 expected(= 上一跳)。
/// WebPkiClientVerifier 只验链;这里补「持证者必须是上一跳」的身份绑定。
pub(crate) fn verify_client_san(
    conn: &tokio_rustls::rustls::ServerConnection,
    expected: &str,
) -> Result<()> {
    let certs = conn
        .peer_certificates()
        .context("tunnel peer presented no client certificate")?;
    let leaf = certs.first().context("empty client certificate chain")?;
    let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|e| anyhow::anyhow!("parse tunnel client cert: {e}"))?;
    let ok = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|ext| {
            ext.value.general_names.iter().any(|n| {
                matches!(n, x509_parser::extensions::GeneralName::DNSName(d) if *d == expected)
            })
        })
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        anyhow::bail!("tunnel client cert SAN does not match previous hop {expected}")
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
        let tcp = tokio::time::timeout(HANDSHAKE_TIMEOUT, TcpStream::connect(addr))
            .await
            .with_context(|| format!("tunnel tls tcp connect {addr} timed out"))?
            .with_context(|| format!("tunnel tls tcp connect {addr}"))?;
        crate::relay::set_nodelay(&tcp);
        let tls = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            self.connector.connect(self.dial_sni.clone(), tcp),
        )
        .await
        .context("tunnel tls client handshake timed out")?
        .context("tunnel tls client handshake")?;
        Ok(Box::new(tls))
    }

    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>> {
        let expect = self
            .expect_client_san
            .clone()
            .context("entry hop (ordinal 0) must not bind a tunnel listener")?;
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| format!("tunnel tls bind {addr}"))?;
        Ok(Box::new(TlsTunnelListener {
            inner: l,
            acceptor: self.acceptor.clone(),
            expect_client_san: expect,
        }))
    }
}

struct TlsTunnelListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
    expect_client_san: String,
}

#[tonic::async_trait]
impl TunnelListener for TlsTunnelListener {
    async fn accept_pending(&mut self) -> Result<Box<dyn PendingHop>> {
        let (tcp, _) = self.inner.accept().await.context("tunnel tls tcp accept")?;
        crate::relay::set_nodelay(&tcp);
        Ok(Box::new(TlsPendingHop {
            tcp,
            acceptor: self.acceptor.clone(),
            expect_client_san: self.expect_client_san.clone(),
        }))
    }

    fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.inner.local_addr()?)
    }
}

struct TlsPendingHop {
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    expect_client_san: String,
}

#[tonic::async_trait]
impl PendingHop for TlsPendingHop {
    async fn handshake(self: Box<Self>) -> Result<TunnelConn> {
        // 握手超时:半开连接(连 TCP 不发 TLS 数据)否则会永久挂住本握手 task(已移出 accept loop)。
        let tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.acceptor.accept(self.tcp))
            .await
            .context("tunnel tls server handshake timed out")?
            .context("tunnel tls server handshake")?;
        verify_client_san(tls.get_ref().1, &self.expect_client_san)?;
        Ok(Box::new(tls))
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

    /// 同 CA、链合法、但 client SAN 指向 hop-7 而非上一跳 hop-0:
    /// hop-1 server 必须在握手后拒绝(SAN 校验)。防同 CA 凭据横向连入。
    #[tokio::test]
    async fn tls_server_rejects_client_cert_with_wrong_hop_san() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        // hop-0 目录:client SAN 伪造为 hop-7;hop-1 目录正常。
        crate::tunnel::testutil::write_hop_creds_matrix(&data_dir, 9, &[(0, 7), (1, 1)]).await;

        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).unwrap();
        let client_t = TlsTransport::load(&data_dir, &ctx(9, 0)).unwrap();
        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client = tokio::spawn(async move {
            // 链验证在握手层通过,dial 可能成功;server 在 accept 内做 SAN 校验后拒绝。
            let _ = client_t.dial(&addr.to_string()).await;
        });
        assert!(
            listener.accept().await.is_err(),
            "client SAN 不是上一跳(hop-0)必须被拒"
        );
        let _ = client.await;
    }

    /// entry(self_ordinal=0)没有上一跳,不允许 bind 隧道 listener(防御)。
    #[tokio::test]
    async fn tls_entry_hop_must_not_bind() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let t = TlsTransport::load(&data_dir, &ctx(9, 0)).unwrap();
        assert!(t.bind("127.0.0.1:0").await.is_err());
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

    /// 半开连接(连 TCP 后永不发 TLS 握手数据)必须被握手超时断开,不能永久挂死
    /// 整个 hop 的 accept loop。虚拟时钟在 runtime idle 时自动推进过 HANDSHAKE_TIMEOUT,
    /// accept 应返回 Err(超时)而非永久挂起。
    #[tokio::test(start_paused = true)]
    async fn tls_accept_times_out_on_half_open_handshake() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        write_hop_creds_pair(&data_dir, 9, 0, 1).await;
        let server_t = TlsTransport::load(&data_dir, &ctx(9, 1)).unwrap();
        let mut listener = server_t.bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // client 连上 TCP 但永不发握手数据,保持半开。
        let client = tokio::spawn(async move {
            let _s = tokio::net::TcpStream::connect(addr).await.unwrap();
            std::future::pending::<()>().await;
        });

        let r = listener.accept().await;
        assert!(r.is_err(), "半开握手必须超时断开,而非永久挂起");
        client.abort();
    }
}
