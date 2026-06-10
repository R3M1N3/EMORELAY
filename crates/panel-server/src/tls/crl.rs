//! 吊销列表(P3a)。fingerprint 集合;register 时拒已吊销证书。
//! T3 给内存版 + 文件加载;吊销落盘(revoke)在 Task 6 加。
use std::collections::HashSet;
use std::path::Path;
use std::sync::RwLock;

#[derive(Default)]
pub struct Crl {
    revoked: RwLock<HashSet<String>>,
}

impl Crl {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 crl.json(JSON 数组 of fingerprint)加载。
    ///
    /// 区分两种情况,绝不静默 fail-open:
    /// - 文件**不存在** → 空集合(首次启动正常路径)。
    /// - 文件**存在但读取/解析失败**(损坏) → 大声 `tracing::error!` 告警并返回空集合。
    ///   P3a 阶段 CRL 尚未做 boot-blocking 强制,单个损坏文件不应让面板崩溃;但必须
    ///   显式告警「已吊销证书可能被重新接受」,运维需立即修复,而非悄无声息地解除吊销。
    pub fn load(path: &str) -> Self {
        if !Path::new(path).exists() {
            return Self::default();
        }
        let revoked = match std::fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Vec<String>>(&s) {
                Ok(v) => v.into_iter().collect(),
                Err(e) => {
                    tracing::error!(
                        path,
                        error = ?e,
                        "CRL file exists but is unreadable/corrupt — revoked certs may be accepted until fixed"
                    );
                    HashSet::new()
                }
            },
            Err(e) => {
                tracing::error!(
                    path,
                    error = ?e,
                    "CRL file exists but is unreadable/corrupt — revoked certs may be accepted until fixed"
                );
                HashSet::new()
            }
        };
        Self {
            revoked: RwLock::new(revoked),
        }
    }

    pub fn is_revoked(&self, fingerprint: &str) -> bool {
        self.revoked.read().unwrap().contains(fingerprint)
    }

    /// 把 fingerprint 加入吊销集并持久化到 path(JSON 数组)。
    pub fn revoke(&self, fingerprint: &str, path: &str) -> anyhow::Result<()> {
        let mut set = self.revoked.write().unwrap();
        set.insert(fingerprint.to_string());
        let snapshot: Vec<&String> = set.iter().collect();
        let json = serde_json::to_string(&snapshot)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}
