use anyhow::{Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub node_id: i64,
    pub control_endpoint: String,
    pub token: String,
    pub state_path: String,
    /// 自签 CA 路径(PEM)。endpoint 是 https:// 时:
    /// - Some → 用它验证 server cert(开发模式 + 自签 CA)
    /// - None → 走系统根证书(生产 + 真实证书,需 tonic tls-roots feature)
    pub grpc_ca_cert: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _ = dotenvy::dotenv();
        let node_id = env::var("AGENT_NODE_ID")
            .context("AGENT_NODE_ID is required")?
            .parse::<i64>()
            .context("AGENT_NODE_ID must be a positive integer")?;
        if node_id <= 0 {
            anyhow::bail!("AGENT_NODE_ID must be a positive integer");
        }
        let control_endpoint = env::var("AGENT_CONTROL_ENDPOINT")
            .context("AGENT_CONTROL_ENDPOINT is required")?;
        let token = env::var("AGENT_TOKEN").context("AGENT_TOKEN is required")?;
        if token.is_empty() {
            anyhow::bail!("AGENT_TOKEN must not be empty");
        }
        let state_path =
            env::var("AGENT_STATE_PATH").unwrap_or_else(|_| "./agent-state.json".into());
        let grpc_ca_cert = env::var("AGENT_GRPC_CA_CERT").ok().filter(|s| !s.is_empty());
        Ok(Self {
            node_id,
            control_endpoint,
            token,
            state_path,
            grpc_ca_cert,
        })
    }
}
