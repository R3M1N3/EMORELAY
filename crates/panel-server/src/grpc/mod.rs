pub mod commands;
pub mod dispatcher;
pub mod service;
pub mod session;

use anyhow::{Context, Result};
use emorelay_common::control::v1::control_plane_server::ControlPlaneServer;
use std::net::SocketAddr;
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tracing::{info, warn};

use crate::state::AppState;

/// gRPC metadata key for session_token 鉴权。Agent 必须用同名 key 携带。
pub const SESSION_METADATA_KEY: &str = "x-emorelay-session";

pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
    let tls_cert = state.config.grpc_tls_cert.clone();
    let tls_key = state.config.grpc_tls_key.clone();
    let svc = ControlPlaneServer::new(service::ControlPlaneImpl::new(state));

    let mut builder = Server::builder();
    match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let cert = std::fs::read(&cert_path)
                .with_context(|| format!("read PANEL_GRPC_TLS_CERT: {cert_path}"))?;
            let key = std::fs::read(&key_path)
                .with_context(|| format!("read PANEL_GRPC_TLS_KEY: {key_path}"))?;
            let identity = Identity::from_pem(cert, key);
            builder = builder
                .tls_config(ServerTlsConfig::new().identity(identity))
                .context("apply gRPC TLS config")?;
            info!(%addr, "grpc control plane listening (TLS)");
        }
        (None, None) => {
            warn!(
                %addr,
                "grpc control plane running in PLAINTEXT (set PANEL_GRPC_TLS_CERT and \
                 PANEL_GRPC_TLS_KEY to enable TLS — strongly recommended for production)"
            );
        }
        _ => {
            anyhow::bail!(
                "PANEL_GRPC_TLS_CERT and PANEL_GRPC_TLS_KEY must both be set or both be empty"
            );
        }
    }

    builder
        .add_service(svc)
        .serve(addr)
        .await
        .context("grpc serve")?;
    Ok(())
}
