//! 隧道 transport 抽象(P3b)。三实现(TCP/TLS/WSS)对上层 TunnelTask 同形:
//! dial 连下一跳、bind+accept 被上一跳连入,连接统一是 boxed 双向字节流。
use anyhow::Result;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};

/// 隧道连接:双向字节流。TLS/WSS 在 transport 内完成握手,对上层透明。
pub trait TunnelStream: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> TunnelStream for T {}

pub type TunnelConn = Box<dyn TunnelStream>;

#[tonic::async_trait]
pub trait TunnelTransport: Send + Sync {
    /// 主动连下一跳(entry/mid)。addr 形如 "1.2.3.4:30001"。
    async fn dial(&self, addr: &str) -> Result<TunnelConn>;
    /// 监听被上一跳连入(mid/exit)。
    async fn bind(&self, addr: &str) -> Result<Box<dyn TunnelListener>>;
}

#[tonic::async_trait]
pub trait TunnelListener: Send {
    async fn accept(&mut self) -> Result<TunnelConn>;
    /// 实际监听地址(测试 bind :0 时取真实端口)。
    fn local_addr(&self) -> Result<SocketAddr>;
}
