use std::sync::Arc;

use crate::{
    config::Config,
    grpc::{dispatcher::CommandDispatcher, session::SessionRegistry},
};
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: SqlitePool,
    pub sessions: Arc<SessionRegistry>,
    pub dispatcher: Arc<CommandDispatcher>,
    pub ca: std::sync::Arc<crate::tls::ca::CaBundle>,
    pub crl: std::sync::Arc<crate::tls::crl::Crl>,
    /// 节点变更事件广播(SSE 实时推送):载荷为发生变更的 node_id,SSE 处理器据此
    /// 拉取该节点快照推给已连接的 admin。register/heartbeat/node_stats/掉线 sweeper 发布。
    pub node_events: Arc<tokio::sync::broadcast::Sender<i64>>,
}

impl AppState {
    /// 发布一个节点变更事件(无订阅者时静默忽略)。
    pub fn publish_node_event(&self, node_id: i64) {
        let _ = self.node_events.send(node_id);
    }
}
