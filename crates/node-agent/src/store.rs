use anyhow::{Context, Result};
use emorelay_common::control::v1::Rule;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tracing::info;

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
