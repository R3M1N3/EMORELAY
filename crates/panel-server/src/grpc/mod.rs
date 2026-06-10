pub mod commands;
pub mod dispatcher;
pub mod service;
pub mod session;
pub mod tunnel_dispatch;
pub mod tunnel_split;

use anyhow::{Context, Result};
use emorelay_common::control::v1::control_plane_server::ControlPlaneServer;
use std::net::SocketAddr;
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
    let mut builder = Server::builder();

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
