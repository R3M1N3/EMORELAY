pub mod commands;
pub mod dispatcher;
pub mod service;
pub mod session;
pub mod tunnel_dispatch;
pub mod tunnel_split;

use anyhow::{Context, Result};
use emorelay_common::control::v1::control_plane_server::ControlPlaneServer;
use std::net::SocketAddr;
use std::time::Duration;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tracing::{info, warn};

use crate::state::AppState;

/// gRPC metadata key for session_token 鉴权。Agent 必须用同名 key 携带。
pub const SESSION_METADATA_KEY: &str = "x-emorelay-session";

/// gRPC 控制面 TLS 模式。默认 mTLS;dev 逃生阀退 plaintext。
pub enum GrpcTlsMode {
    Mtls,
    Plaintext,
}

pub fn tls_mode_for(dev_disable_mtls: bool) -> GrpcTlsMode {
    if dev_disable_mtls {
        GrpcTlsMode::Plaintext
    } else {
        GrpcTlsMode::Mtls
    }
}

pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
    let mode = tls_mode_for(state.config.dev_disable_mtls);
    let ca = state.ca.clone();
    let svc = ControlPlaneServer::new(service::ControlPlaneImpl::new(state));
    // 服务端存活探测,对称镜像 client(node-agent control.rs)的 keepalive。
    // Agent 面向公网 NAT 节点,连接可能被防火墙黑洞或 NAT 空闲表项回收而静默死亡
    // (无 FIN/RST)。仅 client 单侧探活不够:当 agent 静默死亡且其 dispatch channel
    // 已满时,server 侧 reconcile 重放任务的 send().await 会永不返回 Err,直到 OS TCP
    // keepalive(Linux 默认 ~2h),期间持有的 per-node 锁阻塞同 node 的 delete/HTTP DELETE。
    // server 主动 HTTP/2 PING 探活 → 死连接尽快断开 → rx drop → send().await 返 Err →
    // 重放任务退出并释放锁。等待界限从 ~2h 降到探测周期级。
    let mut builder = Server::builder()
        .http2_keepalive_interval(Some(Duration::from_secs(20)))
        .http2_keepalive_timeout(Some(Duration::from_secs(10)));

    match mode {
        GrpcTlsMode::Mtls => {
            // server identity = 内置 server 证书;client_ca_root = 内置 CA(强制 client cert)。
            let identity = Identity::from_pem(
                ca.server_cert_pem.as_bytes(),
                ca.server_key_pem.as_bytes(),
            );
            let tls_cfg = ServerTlsConfig::new()
                .identity(identity)
                .client_ca_root(Certificate::from_pem(ca.ca_pem.as_bytes()));
            builder = builder.tls_config(tls_cfg).context("apply gRPC mTLS config")?;
            info!(%addr, "grpc control plane listening (built-in CA, mTLS enforced)");
        }
        GrpcTlsMode::Plaintext => {
            warn!(%addr, "grpc control plane PLAINTEXT (PANEL_DEV_DISABLE_MTLS set — dev only)");
        }
    }

    builder.add_service(svc).serve(addr).await.context("grpc serve")?;
    Ok(())
}
