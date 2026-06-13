use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy)]
pub struct SessionInfo {
    pub node_id: i64,
    pub expires_at_unix: i64,
}

/// 进程内 session_token → SessionInfo 缓存。
///
/// 注意：纯内存设计，panel-server 重启后所有 session 失效，Agent 需要重新 Register。
/// agent_sessions 表仅作审计用途（写不读）。
#[derive(Default)]
pub struct SessionRegistry {
    sessions: RwLock<HashMap<String, SessionInfo>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, token: String, info: SessionInfo) {
        self.sessions.write().unwrap().insert(token, info);
    }

    /// 查询并隐式过期；过期则不返回也不主动清理（懒清理）。
    pub fn verify(&self, token: &str, now_unix: i64) -> Option<SessionInfo> {
        let map = self.sessions.read().unwrap();
        map.get(token).copied().filter(|s| s.expires_at_unix > now_unix)
    }

    /// 失效某 node 的全部 session(吊销/删除节点后立即生效,不等 24h 过期)。
    pub fn revoke_node(&self, node_id: i64) {
        self.sessions.write().unwrap().retain(|_, s| s.node_id != node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoke_node_drops_all_its_sessions() {
        let reg = SessionRegistry::new();
        reg.insert("tok-a".into(), SessionInfo { node_id: 1, expires_at_unix: i64::MAX });
        reg.insert("tok-b".into(), SessionInfo { node_id: 2, expires_at_unix: i64::MAX });
        reg.revoke_node(1);
        assert!(reg.verify("tok-a", 0).is_none(), "node1 session 应被吊销");
        assert!(reg.verify("tok-b", 0).is_some(), "node2 session 不受影响");
    }
}
