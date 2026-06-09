use anyhow::{anyhow, Context, Result};
use emorelay_common::control::v1::{
    control_plane_client::ControlPlaneClient, Command, HeartbeatRequest, NodeStatsBatch,
    RegisterRequest, RuleStatsBatch, SubscribeRequest,
};
use tokio_stream::Stream;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Streaming};
use tracing::info;

/// 必须与 panel-server::grpc::SESSION_METADATA_KEY 同名。
const SESSION_METADATA_KEY: &str = "x-emorelay-session";

pub struct ControlClient {
    client: ControlPlaneClient<Channel>,
    node_id: i64,
    token: String,
    session_token: Option<String>,
}

impl ControlClient {
    /// 连接 control plane。endpoint 是 `https://` 时启用 TLS:
    /// - `ca_cert` Some → 用它(自签 CA)校验 server
    /// - `ca_cert` None → 走系统根证书(tls-roots feature)
    /// endpoint 是 `http://` 时走 plaintext(仅推荐 dev)。
    pub async fn connect(
        endpoint: String,
        node_id: i64,
        token: String,
        ca_cert: Option<String>,
    ) -> Result<Self> {
        let mut ep: Endpoint =
            Channel::from_shared(endpoint.clone()).context("invalid AGENT_CONTROL_ENDPOINT")?;
        if ep.uri().scheme_str() == Some("https") {
            let mut tls = ClientTlsConfig::new();
            if let Some(ca_path) = ca_cert {
                let pem = std::fs::read(&ca_path)
                    .with_context(|| format!("read AGENT_GRPC_CA_CERT: {ca_path}"))?;
                tls = tls.ca_certificate(Certificate::from_pem(pem));
            } else {
                // 走系统根证书(tls-roots),默认 ALPN h2。
                tls = tls.with_enabled_roots();
            }
            ep = ep.tls_config(tls).context("apply gRPC TLS config")?;
            info!(endpoint = %endpoint, "agent control plane: TLS enabled");
        } else {
            info!(endpoint = %endpoint, "agent control plane: plaintext");
        }
        let channel = ep.connect().await.context("connect to control plane")?;
        Ok(Self {
            client: ControlPlaneClient::new(channel),
            node_id,
            token,
            session_token: None,
        })
    }

    pub async fn register(&mut self) -> Result<()> {
        let resp = self
            .client
            .register(RegisterRequest {
                node_id: self.node_id,
                agent_token: self.token.clone(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            })
            .await
            .context("register rpc")?;
        let inner = resp.into_inner();
        info!(
            node_id = self.node_id,
            expires_at = inner.expires_at_unix,
            "registered with control plane"
        );
        self.session_token = Some(inner.session_token);
        Ok(())
    }

    fn auth_request<T>(&self, body: T) -> Result<Request<T>> {
        let token = self
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow!("agent not registered yet"))?;
        let mut req = Request::new(body);
        let value = MetadataValue::try_from(token).context("session token → metadata value")?;
        req.metadata_mut().insert(SESSION_METADATA_KEY, value);
        Ok(req)
    }

    pub async fn heartbeat(&mut self, cpu: f64, mem: f64, load: f64) -> Result<()> {
        let req = self.auth_request(HeartbeatRequest {
            node_id: self.node_id,
            cpu_usage: cpu,
            memory_usage: mem,
            load_average: load,
        })?;
        self.client.heartbeat(req).await.context("heartbeat rpc")?;
        Ok(())
    }

    pub async fn subscribe_commands(&mut self) -> Result<Streaming<Command>> {
        let req = self.auth_request(SubscribeRequest {
            node_id: self.node_id,
        })?;
        let resp = self
            .client
            .subscribe_commands(req)
            .await
            .context("subscribe rpc")?;
        Ok(resp.into_inner())
    }

    pub async fn report_rule_stats<S>(&mut self, batches: S) -> Result<()>
    where
        S: Stream<Item = RuleStatsBatch> + Send + 'static,
    {
        let token = self
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow!("agent not registered yet"))?;
        let mut req = Request::new(batches);
        let value = MetadataValue::try_from(token).context("session token → metadata value")?;
        req.metadata_mut().insert(SESSION_METADATA_KEY, value);
        self.client
            .report_rule_stats(req)
            .await
            .context("report_rule_stats rpc")?;
        Ok(())
    }

    pub async fn report_node_stats<S>(&mut self, batches: S) -> Result<()>
    where
        S: Stream<Item = NodeStatsBatch> + Send + 'static,
    {
        let token = self
            .session_token
            .as_deref()
            .ok_or_else(|| anyhow!("agent not registered yet"))?;
        let mut req = Request::new(batches);
        let value = MetadataValue::try_from(token).context("session token → metadata value")?;
        req.metadata_mut().insert(SESSION_METADATA_KEY, value);
        self.client
            .report_node_stats(req)
            .await
            .context("report_node_stats rpc")?;
        Ok(())
    }
}
