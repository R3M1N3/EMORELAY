use anyhow::{anyhow, Context, Result};
use emorelay_common::control::v1::{
    control_plane_client::ControlPlaneClient, Command, HeartbeatRequest, NodeStatsBatch, ProbeResult,
    RegisterRequest, RuleStatsBatch, SubscribeRequest,
};
use std::time::Duration;
use tokio_stream::Stream;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::{Request, Streaming};
use tracing::{info, warn};

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
    /// - `ca_cert` Some → 用它(自签 CA)校验 server;None → 走系统根证书(tls-roots feature)
    /// - `client_cert`+`client_key` 同时 Some → 启用 mTLS,带 client identity 证明自己
    ///   (panel-server 配 PANEL_GRPC_TLS_CLIENT_CA 时强制要求)
    /// endpoint 是 `http://` 时走 plaintext(仅推荐 dev)。
    pub async fn connect(
        endpoint: String,
        node_id: i64,
        token: String,
        ca_cert: Option<String>,
        client_cert: Option<String>,
        client_key: Option<String>,
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
            // mTLS client identity:cert+key 必须同时给,否则视为单向 TLS。
            let mtls = match (client_cert.as_deref(), client_key.as_deref()) {
                (Some(c), Some(k)) => {
                    let cert = std::fs::read(c)
                        .with_context(|| format!("read AGENT_GRPC_CLIENT_CERT: {c}"))?;
                    let key = std::fs::read(k)
                        .with_context(|| format!("read AGENT_GRPC_CLIENT_KEY: {k}"))?;
                    tls = tls.identity(Identity::from_pem(cert, key));
                    true
                }
                (None, None) => false,
                _ => anyhow::bail!(
                    "AGENT_GRPC_CLIENT_CERT and AGENT_GRPC_CLIENT_KEY must both be set or both empty"
                ),
            };
            ep = ep.tls_config(tls).context("apply gRPC TLS config")?;
            info!(endpoint = %endpoint, mtls, "agent control plane: TLS enabled");
        } else {
            if client_cert.is_some() || client_key.is_some() {
                warn!(
                    endpoint = %endpoint,
                    "AGENT_GRPC_CLIENT_CERT/KEY configured but endpoint is plaintext (http://); \
                     mTLS NOT in effect — change endpoint to https:// or remove client cert env"
                );
            }
            info!(endpoint = %endpoint, "agent control plane: plaintext");
        }
        // 控制连接存活/超时:Agent 直连公网且面向 NAT 节点,底层连接可能被防火墙黑洞
        // 或 NAT 空闲表项回收而静默死亡。无 keepalive 时主循环的 command_stream.message()
        // 与 heartbeat() 会永不返回,run_session 既不 Ok 也不 Err,旁路掉 RETRY_BACKOFF 重连。
        // HTTP/2 PING 探活(含空闲)+ connect 超时让死连接尽快报错,触发既有重连。
        ep = ep
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .http2_keep_alive_interval(Duration::from_secs(20))
            .keep_alive_timeout(Duration::from_secs(10))
            .keep_alive_while_idle(true);
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

    pub async fn heartbeat(&mut self, cpu: f64, mem: f64, load: f64, ipv4_cap: i32, ipv6_cap: i32) -> Result<()> {
        let req = self.auth_request(HeartbeatRequest {
            node_id: self.node_id,
            cpu_usage: cpu,
            memory_usage: mem,
            load_average: load,
            ipv4_capability: ipv4_cap,
            ipv6_capability: ipv6_cap,
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

    /// 取一个轻量探测回报器(克隆底层 channel + session token),交给 spawn 出去的探测
    /// 任务异步回报,不占用主循环的 &mut self。未注册时返回 None。
    pub fn probe_reporter(&self) -> Option<ProbeReporter> {
        self.session_token.as_ref().map(|t| ProbeReporter {
            client: self.client.clone(),
            session_token: t.clone(),
        })
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

/// 探测结果回报器:持有克隆的 channel,可被 move 进 spawn 的探测任务。
pub struct ProbeReporter {
    client: ControlPlaneClient<Channel>,
    session_token: String,
}

impl ProbeReporter {
    pub async fn report(mut self, result: ProbeResult) -> Result<()> {
        let mut req = Request::new(result);
        let value = MetadataValue::try_from(&self.session_token)
            .context("session token → metadata value")?;
        req.metadata_mut().insert(SESSION_METADATA_KEY, value);
        self.client
            .report_probe_result(req)
            .await
            .context("report_probe_result rpc")?;
        Ok(())
    }
}
