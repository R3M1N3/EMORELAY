use chrono::Utc;
use emorelay_common::control::v1::{
    control_plane_server::ControlPlane, Ack, Command, HeartbeatRequest, HeartbeatResponse,
    NodeStatsBatch, RegisterRequest, RegisterResponse, RuleStatsBatch, SubscribeRequest,
};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};

use crate::{
    audit,
    auth::token::{generate_token, hash_token},
    grpc::commands::apply_command,
    grpc::dispatcher::CommandDispatcher,
    grpc::session::SessionInfo,
    grpc::SESSION_METADATA_KEY,
    models::rule::Rule as DbRule,
    state::AppState,
};

/// session_token 有效期。MVP 阶段固定 24h；过期 Agent 自动 re-register。
const SESSION_TTL_HOURS: i64 = 24;

// tonic 生成的 trait 方法签名固定 `Result<..., Status>`，Status ~176 字节，
// 触发 clippy::result_large_err；签名不可改，因此整个 impl 上抑制。
#[allow(clippy::result_large_err)]
pub struct ControlPlaneImpl {
    state: AppState,
}

impl ControlPlaneImpl {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    fn verify_session<T>(&self, req: &Request<T>) -> Result<i64, Status> {
        let raw = req
            .metadata()
            .get(SESSION_METADATA_KEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing session token"))?;
        let info = self
            .state
            .sessions
            .verify(raw, Utc::now().timestamp())
            .ok_or_else(|| Status::unauthenticated("invalid or expired session"))?;
        Ok(info.node_id)
    }
}

#[tonic::async_trait]
impl ControlPlane for ControlPlaneImpl {
    async fn register(
        &self,
        req: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        let req = req.into_inner();

        // 取 nodes 表存储的 token hash。
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT agent_token_hash FROM nodes WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(req.node_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| Status::internal(format!("db: {e}")))?;

        // 安全收紧：unknown_node 与 bad_token 返回同一消息 + 同一状态码，
        // 防止攻击者通过差异化错误信息枚举存在的 node_id。
        let Some((stored_hash,)) = row else {
            audit::record(
                &self.state.pool,
                None,
                "agent.register",
                Some("node"),
                Some(req.node_id),
                Some(&format!("version={}", req.version)),
                false,
                Some("unknown_node"),
            )
            .await;
            return Err(Status::permission_denied("permission denied"));
        };
        if hash_token(&req.agent_token) != stored_hash {
            audit::record(
                &self.state.pool,
                None,
                "agent.register",
                Some("node"),
                Some(req.node_id),
                Some(&format!("version={}", req.version)),
                false,
                Some("bad_token"),
            )
            .await;
            return Err(Status::permission_denied("permission denied"));
        }

        // 颁发 session_token：明文进内存，hash 落 agent_sessions 表（审计）。
        let session_token = generate_token();
        let session_hash = hash_token(&session_token);
        let expires_at_unix = Utc::now().timestamp() + SESSION_TTL_HOURS * 3600;

        if let Err(e) = sqlx::query(
            "INSERT INTO agent_sessions (node_id, session_token_hash) VALUES (?, ?)",
        )
        .bind(req.node_id)
        .bind(&session_hash)
        .execute(&self.state.pool)
        .await
        {
            warn!(error = ?e, "failed to persist agent_sessions row");
        }

        self.state.sessions.insert(
            session_token.clone(),
            SessionInfo {
                node_id: req.node_id,
                expires_at_unix,
            },
        );

        // 标记 node 在线。
        let _ = sqlx::query(
            "UPDATE nodes SET status = 'online', last_seen_at = datetime('now'), \
             updated_at = datetime('now') WHERE id = ?",
        )
        .bind(req.node_id)
        .execute(&self.state.pool)
        .await;

        audit::record(
            &self.state.pool,
            None,
            "agent.register",
            Some("node"),
            Some(req.node_id),
            Some(&format!("version={}", req.version)),
            true,
            None,
        )
        .await;

        info!(node_id = req.node_id, "agent registered");

        Ok(Response::new(RegisterResponse {
            session_token,
            expires_at_unix,
        }))
    }

    async fn heartbeat(
        &self,
        req: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let session_node_id = self.verify_session(&req)?;
        let inner = req.into_inner();
        if inner.node_id != session_node_id {
            return Err(Status::permission_denied("session/node mismatch"));
        }

        sqlx::query(
            "UPDATE nodes SET cpu_usage = ?, memory_usage = ?, load_average = ?, \
             status = 'online', last_seen_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(inner.cpu_usage)
        .bind(inner.memory_usage)
        .bind(inner.load_average)
        .bind(inner.node_id)
        .execute(&self.state.pool)
        .await
        .map_err(|e| Status::internal(format!("db: {e}")))?;

        Ok(Response::new(HeartbeatResponse {
            server_time_unix: Utc::now().timestamp(),
        }))
    }

    type SubscribeCommandsStream =
        Pin<Box<dyn Stream<Item = Result<Command, Status>> + Send + 'static>>;

    async fn subscribe_commands(
        &self,
        req: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeCommandsStream>, Status> {
        let session_node_id = self.verify_session(&req)?;
        let inner = req.into_inner();
        if inner.node_id != session_node_id {
            return Err(Status::permission_denied("session/node mismatch"));
        }
        let (rx, generation) = self.state.dispatcher.subscribe(inner.node_id);

        // Reconcile：新 channel 建立后立即重放该 node 所有 active 规则。
        // 覆盖断网期间漏掉的 CRUD，让 Agent 重连后与 server 真值对齐。
        let reconciled = match DbRule::list_active_for_node(&self.state.pool, inner.node_id).await {
            Ok(rules) => {
                for rule in &rules {
                    self.state
                        .dispatcher
                        .dispatch(inner.node_id, apply_command(rule));
                }
                rules.len()
            }
            Err(e) => {
                warn!(error = ?e, "reconcile query failed; agent will run with last-known rules");
                0
            }
        };
        info!(node_id = inner.node_id, reconciled, "command stream opened");

        // 用 GuardedStream 包装 receiver:stream 终止 (agent 断连 / 主动 cancel) 时
        // DispatcherGuard::drop -> unsubscribe_if(node_id, generation),清理 channels
        // 表中的 dead sender,防长跑下内存累积。
        let guard = DispatcherGuard {
            dispatcher: self.state.dispatcher.clone(),
            node_id: inner.node_id,
            generation,
        };
        let stream = GuardedStream {
            inner: UnboundedReceiverStream::new(rx).map(Ok),
            _guard: guard,
        };
        Ok(Response::new(Box::pin(stream) as Self::SubscribeCommandsStream))
    }

    async fn report_node_stats(
        &self,
        req: Request<Streaming<NodeStatsBatch>>,
    ) -> Result<Response<Ack>, Status> {
        let session_node_id = self.verify_session(&req)?;
        let mut stream = req.into_inner();
        let mut total_buckets = 0usize;
        while let Some(batch) = stream
            .message()
            .await
            .map_err(|e| Status::internal(format!("stream: {e}")))?
        {
            if batch.node_id != session_node_id {
                return Err(Status::permission_denied("session/node mismatch"));
            }
            for bucket in &batch.buckets {
                if bucket.bucket_at_unix <= 0 {
                    continue;
                }
                let Some(bucket_at) =
                    chrono::DateTime::<chrono::Utc>::from_timestamp(bucket.bucket_at_unix, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                else {
                    continue;
                };

                // 事务包裹：node_stats UPSERT 与 nodes 累加要么都生效要么都 rollback,
                // 与 rule_stats 处理方式一致。
                let mut tx = match self.state.pool.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = ?e, "begin tx failed");
                        continue;
                    }
                };

                // node_stats: CPU/MEM/LOAD 是采样瞬时值,latest 覆盖;
                // rx/tx 是窗口增量,累加 (同 bucket 重传时合并)。
                let upsert = sqlx::query(
                    "INSERT INTO node_stats (node_id, bucket_at, cpu_usage, memory_usage, load_average, rx_bytes, tx_bytes) \
                     VALUES (?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(node_id, bucket_at) DO UPDATE SET \
                        cpu_usage = excluded.cpu_usage, \
                        memory_usage = excluded.memory_usage, \
                        load_average = excluded.load_average, \
                        rx_bytes = rx_bytes + excluded.rx_bytes, \
                        tx_bytes = tx_bytes + excluded.tx_bytes",
                )
                .bind(batch.node_id)
                .bind(&bucket_at)
                .bind(bucket.cpu_usage)
                .bind(bucket.memory_usage)
                .bind(bucket.load_average)
                .bind(bucket.rx_bytes)
                .bind(bucket.tx_bytes)
                .execute(&mut *tx)
                .await;
                if let Err(e) = upsert {
                    warn!(error = ?e, node_id = batch.node_id, "node_stats upsert failed");
                    let _ = tx.rollback().await;
                    continue;
                }

                // nodes 表:CPU/MEM/LOAD 覆盖, rx/tx_total 累加, last_seen_at 刷新.
                // node_stats UPSERT 处已经合并了重传,这里直接累加;
                // 偶发重传会让 nodes.rx_bytes_total 略偏高,可接受 (MVP)。
                let update = sqlx::query(
                    "UPDATE nodes SET \
                        cpu_usage = ?, \
                        memory_usage = ?, \
                        load_average = ?, \
                        rx_bytes_total = rx_bytes_total + ?, \
                        tx_bytes_total = tx_bytes_total + ?, \
                        status = 'online', \
                        last_seen_at = datetime('now'), \
                        updated_at = datetime('now') \
                     WHERE id = ? AND deleted_at IS NULL",
                )
                .bind(bucket.cpu_usage)
                .bind(bucket.memory_usage)
                .bind(bucket.load_average)
                .bind(bucket.rx_bytes)
                .bind(bucket.tx_bytes)
                .bind(batch.node_id)
                .execute(&mut *tx)
                .await;
                if let Err(e) = update {
                    warn!(error = ?e, node_id = batch.node_id, "nodes update failed");
                    let _ = tx.rollback().await;
                    continue;
                }

                if let Err(e) = tx.commit().await {
                    warn!(error = ?e, node_id = batch.node_id, "node_stats commit failed");
                    continue;
                }
                total_buckets += 1;
            }
        }
        info!(node_id = session_node_id, buckets = total_buckets, "node stats persisted");
        Ok(Response::new(Ack {
            ok: true,
            error: String::new(),
        }))
    }

    async fn report_rule_stats(
        &self,
        req: Request<Streaming<RuleStatsBatch>>,
    ) -> Result<Response<Ack>, Status> {
        let session_node_id = self.verify_session(&req)?;
        let mut stream = req.into_inner();
        let mut total_buckets = 0usize;
        while let Some(batch) = stream
            .message()
            .await
            .map_err(|e| Status::internal(format!("stream: {e}")))?
        {
            if batch.node_id != session_node_id {
                return Err(Status::permission_denied("session/node mismatch"));
            }
            for bucket in &batch.buckets {
                // 非法时间戳跳过，避免被合并到 1970-01-01 epoch bucket。
                if bucket.bucket_at_unix <= 0 {
                    continue;
                }
                let Some(bucket_at) =
                    chrono::DateTime::<chrono::Utc>::from_timestamp(bucket.bucket_at_unix, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                else {
                    continue;
                };

                // 事务包裹：rule_stats UPSERT 与 forward_rules 累加要么都生效，
                // 要么都 rollback，确保 series 总和始终等于 current 累计。
                let mut tx = match self.state.pool.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(error = ?e, "begin tx failed");
                        continue;
                    }
                };
                let upsert = sqlx::query(
                    "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes, connection_count, error_count) \
                     VALUES (?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(rule_id, bucket_at) DO UPDATE SET \
                        rx_bytes = rx_bytes + excluded.rx_bytes, \
                        tx_bytes = tx_bytes + excluded.tx_bytes, \
                        connection_count = connection_count + excluded.connection_count, \
                        error_count = error_count + excluded.error_count",
                )
                .bind(bucket.rule_id)
                .bind(&bucket_at)
                .bind(bucket.rx_bytes)
                .bind(bucket.tx_bytes)
                .bind(bucket.connection_count)
                .bind(bucket.error_count)
                .execute(&mut *tx)
                .await;
                if let Err(e) = upsert {
                    warn!(error = ?e, rule_id = bucket.rule_id, "rule_stats upsert failed");
                    let _ = tx.rollback().await;
                    continue;
                }
                let accumulate = sqlx::query(
                    "UPDATE forward_rules SET \
                        rx_bytes = rx_bytes + ?, \
                        tx_bytes = tx_bytes + ?, \
                        connection_count = connection_count + ?, \
                        updated_at = datetime('now') \
                     WHERE id = ? AND deleted_at IS NULL",
                )
                .bind(bucket.rx_bytes)
                .bind(bucket.tx_bytes)
                .bind(bucket.connection_count)
                .bind(bucket.rule_id)
                .execute(&mut *tx)
                .await;
                if let Err(e) = accumulate {
                    warn!(error = ?e, rule_id = bucket.rule_id, "forward_rules accumulate failed");
                    let _ = tx.rollback().await;
                    continue;
                }
                if let Err(e) = tx.commit().await {
                    warn!(error = ?e, rule_id = bucket.rule_id, "stats commit failed");
                    continue;
                }
                total_buckets += 1;
            }
        }
        info!(node_id = session_node_id, buckets = total_buckets, "rule stats persisted");
        Ok(Response::new(Ack {
            ok: true,
            error: String::new(),
        }))
    }
}

/// SubscribeCommands stream 终止时通过 Drop 调 unsubscribe_if,清理 dispatcher
/// channels 表中的 dead sender。generation 字段保证只清理本次订阅 (并发新订阅替换后,
/// 旧 guard Drop 不会误删新 entry)。
struct DispatcherGuard {
    dispatcher: Arc<CommandDispatcher>,
    node_id: i64,
    generation: u64,
}

impl Drop for DispatcherGuard {
    fn drop(&mut self) {
        self.dispatcher.unsubscribe_if(self.node_id, self.generation);
    }
}

/// 把一个 stream 与一个 Drop guard 捆绑;stream 被 Box::pin 后随 client cancel /
/// connection close 一起 drop,guard 随之 drop,执行清理。
struct GuardedStream<S> {
    inner: S,
    _guard: DispatcherGuard,
}

impl<S> Stream for GuardedStream<S>
where
    S: Stream + Unpin,
{
    type Item = S::Item;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
