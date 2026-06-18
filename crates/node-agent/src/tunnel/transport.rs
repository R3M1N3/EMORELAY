//! 隧道 transport 抽象(P3b)。三实现(TCP/TLS/WSS)对上层 TunnelTask 同形:
//! dial 连下一跳、bind+accept 被上一跳连入,连接统一是 boxed 双向字节流。
use anyhow::Result;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};

/// 隧道连接:双向字节流。TLS/WSS 在 transport 内完成握手,对上层透明。
pub trait TunnelStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> TunnelStream for T {}

pub type TunnelConn = Box<dyn TunnelStream>;

/// 隧道建联/握手统一超时:dial 的 TCP connect 与 TLS/WS 握手、accept 的 TLS/WS 握手
/// 各受此上限约束。防半开连接(连 TCP 不发握手数据)永久挂死 hop 的 accept loop
/// (slowloris DoS),以及黑洞下一跳时单连接永久占用 conn permit/fd。
/// 10s 对正常握手极宽松(同机房 <1ms,跨国高延迟链路通常也 <2s)。
pub(crate) const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tonic::async_trait]
pub trait TunnelTransport: Send + Sync {
    /// 主动连下一跳(entry/mid)。addr 形如 "1.2.3.4:30001"。
    async fn dial(&self, addr: &str) -> Result<TunnelConn>;
    /// 监听被上一跳连入(mid/exit)。
    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>>;
}

#[tonic::async_trait]
pub trait TunnelListener: Send {
    /// 只接受 TCP 连接、不做握手,返回待握手句柄。握手(TLS/WS + client SAN 校验)推迟到
    /// per-conn task 内完成(见 task.rs::start_relay_hop),使慢/半开握手不再在 accept loop
    /// 内串行执行,消除接入队头阻塞 DoS(HANDSHAKE_TIMEOUT 只限单连接耗时,不消除串行化)。
    async fn accept_pending(&mut self) -> Result<Box<dyn PendingHop>>;
    /// 实际监听地址(测试 bind :0 时取真实端口)。
    fn local_addr(&self) -> Result<SocketAddr>;
    /// 便捷组合:接 TCP + 完成握手。测试与简单调用方用;生产 hop loop 用
    /// accept_pending 把握手移出 accept loop。
    async fn accept(&mut self) -> Result<TunnelConn> {
        self.accept_pending().await?.handshake().await
    }
}

/// 待握手的入站连接:TCP 已接受,TLS/WS 握手与 client SAN 身份校验延后到此处完成。
/// 由 per-conn task 调用(握手不阻塞 accept loop);握手内部仍受 HANDSHAKE_TIMEOUT 约束。
#[tonic::async_trait]
pub trait PendingHop: Send {
    async fn handshake(self: Box<Self>) -> Result<TunnelConn>;
}
