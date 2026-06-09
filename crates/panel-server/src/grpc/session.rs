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
}
