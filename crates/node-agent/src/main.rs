use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

use node_agent::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    info!(
        node_id = config.node_id,
        endpoint = %config.control_endpoint,
        state_path = %config.state_path,
        "node-agent starting"
    );
    node_agent::run_agent(config).await
}
