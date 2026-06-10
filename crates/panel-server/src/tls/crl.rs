//! 吊销列表(P3a)。fingerprint 集合;register 时拒已吊销证书。
//! T3 给内存版 + 文件加载;吊销落盘(revoke)在 Task 6 加。
use std::collections::HashSet;
use std::sync::RwLock;

#[derive(Default)]
pub struct Crl {
    revoked: RwLock<HashSet<String>>,
}

impl Crl {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 crl.json(JSON 数组 of fingerprint)加载;文件不存在 → 空集合。
    pub fn load(path: &str) -> Self {
        let revoked = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Self {
            revoked: RwLock::new(revoked),
        }
    }

    pub fn is_revoked(&self, fingerprint: &str) -> bool {
        self.revoked.read().unwrap().contains(fingerprint)
    }
}
