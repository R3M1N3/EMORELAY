use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    config::Config,
    error::ApiError,
    grpc::{dispatcher::CommandDispatcher, session::SessionRegistry},
};
use emorelay_common::control::v1::ProbeResult;
use sqlx::SqlitePool;
use tokio::sync::oneshot;

/// probe_waiters 全局上限:同时在途的逐段诊断 probe 数。隧道诊断按跳数 fan-out 放大,
/// 无上限时任意认证用户可借此把内存 map 撑爆(DoS)。达上限后新 register 被拒,诊断
/// handler 映射为 HTTP 429。这是**全局兜底**(跨并发诊断的总闸),配合 diagnose 端点的
/// per-user 限流(主控)——单用户被节流到 burst 3、单次诊断段数 = 跳数+1,正常远不触顶;
/// 触顶即拒绝新诊断而非无界增长。
pub const MAX_PROBE_WAITERS: usize = 64;

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
    /// 逐段诊断的请求-响应等待者:probe_id → oneshot 发送端。REST 诊断处理器注册,
    /// gRPC report_probe_result 命中后投递结果。超时则等待者自行 drop(发送端 send 失败被忽略)。
    pub probe_waiters: Arc<Mutex<HashMap<String, oneshot::Sender<ProbeResult>>>>,
    /// probe_id 单调计数器(进程内唯一即可,配合 node_id 在 Agent 侧无歧义)。
    pub probe_seq: Arc<AtomicU64>,
    /// 失败登录审计的进程内节流器:同 IP 短窗口内只落一条 auth.login 失败审计,
    /// 防止(分布式)爆破把审计表与「最近 N 条」视图刷满。详见 [`crate::audit::LoginAuditThrottle`]。
    pub login_audit_throttle: Arc<crate::audit::LoginAuditThrottle>,
}

impl AppState {
    /// 发布一个节点变更事件(无订阅者时静默忽略)。
    pub fn publish_node_event(&self, node_id: i64) {
        let _ = self.node_events.send(node_id);
    }

    /// 注册一个探测等待者,返回 (probe_id, 接收端)。
    /// 在途等待者达 [`MAX_PROBE_WAITERS`] 时拒绝,返回 [`ApiError::TooManyRequests`]
    /// 防无界增长;插入与上限检查在同一把锁内完成,避免并发越限。
    pub fn register_probe(
        &self,
    ) -> Result<(String, oneshot::Receiver<ProbeResult>), ApiError> {
        let id = format!("p{}", self.probe_seq.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        {
            let mut waiters = self.probe_waiters.lock().unwrap();
            if waiters.len() >= MAX_PROBE_WAITERS {
                return Err(ApiError::TooManyRequests("诊断繁忙，请稍后再试".to_string()));
            }
            waiters.insert(id.clone(), tx);
        }
        Ok((id, rx))
    }

    /// 投递探测结果给对应等待者(已超时移除则忽略)。
    pub fn resolve_probe(&self, result: ProbeResult) {
        if let Some(tx) = self.probe_waiters.lock().unwrap().remove(&result.probe_id) {
            let _ = tx.send(result);
        }
    }

    /// 放弃一个探测等待者(超时清理,防 map 泄漏)。
    pub fn cancel_probe(&self, probe_id: &str) {
        self.probe_waiters.lock().unwrap().remove(probe_id);
    }
}
