//! 多跳隧道数据面(P3b)。模块边界:transport(链路) / task(角色编排) /
//! frame(线协议) / creds(凭据落盘)。RuleManager 是唯一调用入口。
pub mod tcp_transport;
pub mod transport;

use anyhow::Result;
use emorelay_common::control::v1::TunnelContext;
use std::sync::Arc;

use self::tcp_transport::TcpTransport;
use self::transport::TunnelTransport;

/// 按 TunnelContext.transport 构建 transport。data_dir 用于 tls/wss 读隧道凭据。
pub fn make_transport(ctx: &TunnelContext, data_dir: &str) -> Result<Arc<dyn TunnelTransport>> {
    let _ = data_dir; // tls/wss 落地(后续 Task)前未用。
    match ctx.transport.as_str() {
        "tcp" => Ok(Arc::new(TcpTransport)),
        "tls" => anyhow::bail!("tls tunnel transport not implemented yet (later task)"),
        "wss" => anyhow::bail!("wss tunnel transport not implemented yet (later task)"),
        other => anyhow::bail!("unknown tunnel transport: {other}"),
    }
}
