use anyhow::{Context, Result};
use emorelay_common::control::v1::Rule;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tracing::info;

/// P3b 数据面起 tunnel 上下文随规则持久化,断网重启恢复隧道角色。
/// 镜像 proto TunnelContext(prost 类型未派生 Serialize)。
#[derive(Serialize, Deserialize, Clone)]
struct TunnelJson {
    tunnel_id: i64,
    role: i32,
    next_hop_addr: String,
    next_hop_inter_port: u32,
    self_inter_port: u32,
    transport: String,
    #[serde(default)]
    self_ordinal: u32,
}

/// JSON 镜像 prost Rule，因为 prost 生成的类型未派生 Serialize。
/// 字段集与 proto Rule 严格一一对应；新增字段时两侧必须同步。
#[derive(Serialize, Deserialize)]
struct RuleJson {
    id: i64,
    protocol: String,
    listen_ip: String,
    listen_port: u32,
    target_host: String,
    target_port: u32,
    enabled: bool,
    /// P2 新增。`#[serde(default)]` 兼容旧版 agent-state.json(缺字段 → 0 = 不限速)。
    #[serde(default)]
    bandwidth_mbps: i64,
    /// P3b 数据面新增。`#[serde(default)]` 兼容旧版 agent-state.json(缺字段 → 非隧道规则)。
    #[serde(default)]
    tunnel: Option<TunnelJson>,
}

impl From<&Rule> for RuleJson {
    fn from(r: &Rule) -> Self {
        Self {
            id: r.id,
            protocol: r.protocol.clone(),
            listen_ip: r.listen_ip.clone(),
            listen_port: r.listen_port,
            target_host: r.target_host.clone(),
            target_port: r.target_port,
            enabled: r.enabled,
            bandwidth_mbps: r.bandwidth_mbps,
            tunnel: r.tunnel.as_ref().map(|t| TunnelJson {
                tunnel_id: t.tunnel_id,
                role: t.role,
                next_hop_addr: t.next_hop_addr.clone(),
                next_hop_inter_port: t.next_hop_inter_port,
                self_inter_port: t.self_inter_port,
                transport: t.transport.clone(),
                self_ordinal: t.self_ordinal,
            }),
        }
    }
}

impl From<RuleJson> for Rule {
    fn from(r: RuleJson) -> Self {
        Self {
            id: r.id,
            protocol: r.protocol,
            listen_ip: r.listen_ip,
            listen_port: r.listen_port,
            target_host: r.target_host,
            target_port: r.target_port,
            enabled: r.enabled,
            bandwidth_mbps: r.bandwidth_mbps,
            tunnel: r.tunnel.map(|t| emorelay_common::control::v1::TunnelContext {
                tunnel_id: t.tunnel_id,
                role: t.role,
                next_hop_addr: t.next_hop_addr,
                next_hop_inter_port: t.next_hop_inter_port,
                self_inter_port: t.self_inter_port,
                transport: t.transport,
                self_ordinal: t.self_ordinal,
            }),
        }
    }
}

/// 本地规则状态持久化。MVP 用单个 JSON 文件；后续可换 SQLite。
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub async fn load(&self) -> Result<Vec<Rule>> {
        match fs::read(&self.path).await {
            Ok(bytes) => {
                let rules: Vec<RuleJson> = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                info!(
                    count = rules.len(),
                    path = %self.path.display(),
                    "loaded persisted rules"
                );
                Ok(rules.into_iter().map(Into::into).collect())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!(path = %self.path.display(), "no persisted state file; starting empty");
                Ok(Vec::new())
            }
            Err(e) => Err(e).context("read state file"),
        }
    }

    /// 原子写：先写 .tmp 再 rename，避免崩溃时写一半。
    pub async fn save(&self, rules: &[Rule]) -> Result<()> {
        let json_rules: Vec<RuleJson> = rules.iter().map(RuleJson::from).collect();
        let bytes = serde_json::to_vec_pretty(&json_rules).context("serialize rules")?;

        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = fs::create_dir_all(parent).await;
            }
        }
        let mut tmp = self.path.clone();
        tmp.as_mut_os_string().push(".tmp");
        fs::write(&tmp, &bytes)
            .await
            .with_context(|| format!("write tmp state: {}", tmp.display()))?;
        fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("rename tmp -> {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{TunnelContext, TunnelRole};

    /// save → load 后 tunnel 上下文必须完整还原(P3b 数据面:Agent 断网重启要能恢复隧道角色)。
    #[tokio::test]
    async fn save_load_round_trips_tunnel_context() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        let store = ConfigStore::new(path);

        let rule = Rule {
            id: 5,
            protocol: "tcp".into(),
            listen_ip: "0.0.0.0".into(),
            listen_port: 20000,
            target_host: "9.9.9.9".into(),
            target_port: 443,
            enabled: true,
            bandwidth_mbps: 30,
            tunnel: Some(TunnelContext {
                tunnel_id: 7,
                role: TunnelRole::Mid as i32,
                next_hop_addr: "10.0.0.3".into(),
                next_hop_inter_port: 30002,
                self_inter_port: 30001,
                transport: "tls".into(),
                self_ordinal: 1,
            }),
        };
        store.save(&[rule.clone()]).await.unwrap();
        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].tunnel, rule.tunnel, "tunnel 上下文必须持久化");
    }

    /// 旧版 agent-state.json(无 tunnel 字段)必须能加载(serde default 兼容)。
    #[tokio::test]
    async fn load_legacy_state_without_tunnel_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        tokio::fs::write(
            &path,
            r#"[{"id":1,"protocol":"tcp","listen_ip":"0.0.0.0","listen_port":1000,
                "target_host":"1.1.1.1","target_port":80,"enabled":true,"bandwidth_mbps":0}]"#,
        )
        .await
        .unwrap();
        let loaded = ConfigStore::new(path).load().await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].tunnel.is_none());
    }
}
