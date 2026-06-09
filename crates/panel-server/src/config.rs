use anyhow::{Context, Result};
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub jwt_secret: String,
    pub jwt_expiry_hours: u64,
    pub cors_origin: String,
    pub grpc_bind_addr: String,
    /// gRPC TLS server cert(PEM)。`None` → 走 plaintext。生产强烈建议配。
    pub grpc_tls_cert: Option<String>,
    pub grpc_tls_key: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        Ok(Self {
            bind_addr: env::var("PANEL_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            database_url: env::var("PANEL_DATABASE_URL")
                .context("PANEL_DATABASE_URL is required")?,
            jwt_secret: env::var("PANEL_JWT_SECRET").context("PANEL_JWT_SECRET is required")?,
            jwt_expiry_hours: env::var("PANEL_JWT_EXPIRY_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(24),
            cors_origin: env::var("PANEL_CORS_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:5173".into()),
            grpc_bind_addr: env::var("PANEL_GRPC_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:50051".into()),
            grpc_tls_cert: env::var("PANEL_GRPC_TLS_CERT")
                .ok()
                .filter(|s| !s.is_empty()),
            grpc_tls_key: env::var("PANEL_GRPC_TLS_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
        })
    }
}
