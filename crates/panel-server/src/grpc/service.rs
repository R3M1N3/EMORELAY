use chrono::Utc;
use emorelay_common::control::v1::{
    control_plane_server::ControlPlane, Ack, Command, HeartbeatRequest, HeartbeatResponse,
    NodeStatsBatch, RegisterRequest, RegisterResponse, RuleStatsBatch, SubscribeRequest,
};
use std::pin::Pin;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};

use crate::{
    audit,
    auth::token::{generate_token, hash_token},
    grpc::commands::{apply_command, parse_sqlite_datetime},
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
        let rx = self.state.dispatcher.subscribe(inner.node_id);

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

        let stream = UnboundedReceiverStream::new(rx).map(Ok);
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

                // 累加完成后判断是否触发自动停规则。失败不阻塞后续 bucket。
                if let Err(e) = auto_stop_if_exceeded(&self.state, bucket.rule_id).await {
                    warn!(error = ?e, rule_id = bucket.rule_id, "auto_stop_if_exceeded failed");
                }
            }
        }
        info!(node_id = session_node_id, buckets = total_buckets, "rule stats persisted");
        Ok(Response::new(Ack {
            ok: true,
            error: String::new(),
        }))
    }
}

/// 检查规则是否触发 traffic_limit 或 expires_at 自动停。
/// 行为(对应 plan 第十节"超过总流量限制后,Agent 自动停止该规则并上报状态"):
///   1. SELECT 当前 rule 累计 rx+tx / limit / expires_at / enabled / node_id。
///   2. 若 enabled=1 且(累计 > limit 或 expires_at < now):
///      - UPDATE enabled=0(WHERE enabled=1 原子化,防止并发重复触发)
///      - dispatch ApplyRule(enabled=false) 让 Agent 停 listener。Agent 离线时下次 register reconcile 自动对齐。
///      - audit 记 `rule.auto_stop`,payload 同时写 reason 与 dispatched 标志,方便事后排查到底是 Agent 真停了还是只是 DB 改了等 reconcile。
///   3. 已停或不存在则 no-op。
///
/// **稳定性**:`pub` 仅为 `report_rule_stats` / `spawn_expiry_sweeper` / integration tests 复用,
/// 不属于稳定对外 API,请勿在其他模块直接调用。
pub async fn auto_stop_if_exceeded(
    state: &AppState,
    rule_id: i64,
) -> anyhow::Result<()> {
    let row: Option<(i64, i64, Option<i64>, Option<String>, i64)> = sqlx::query_as(
        "SELECT enabled, rx_bytes + tx_bytes, traffic_limit_bytes, expires_at, node_id \
         FROM forward_rules WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(rule_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((enabled, total, limit, expires_at, node_id)) = row else {
        return Ok(());
    };
    if enabled == 0 {
        return Ok(());
    }

    let traffic_exceeded = matches!(limit, Some(l) if l > 0 && total > l);
    let now_unix = Utc::now().timestamp();
    let expired = expires_at
        .as_deref()
        .map(parse_sqlite_datetime)
        .is_some_and(|ts| ts > 0 && ts <= now_unix);

    if !traffic_exceeded && !expired {
        return Ok(());
    }

    let reason = if traffic_exceeded {
        "traffic_limit_exceeded"
    } else {
        "expired"
    };

    // 原子化:WHERE enabled = 1 保证并发只触发一次。
    let rows = sqlx::query(
        "UPDATE forward_rules SET enabled = 0, updated_at = datetime('now') \
         WHERE id = ? AND enabled = 1 AND deleted_at IS NULL",
    )
    .bind(rule_id)
    .execute(&state.pool)
    .await?;
    if rows.rows_affected() == 0 {
        // 已被别处停了 / 已删,no-op。
        return Ok(());
    }

    // 先 dispatch,再 audit 写入 dispatched 结果,留下"真停 vs 等 reconcile"的可排查痕迹。
    let dispatched = if let Ok(Some(rule)) = DbRule::find_by_id(&state.pool, rule_id).await {
        state.dispatcher.dispatch(node_id, apply_command(&rule))
    } else {
        false
    };
    crate::audit::record(
        &state.pool,
        None,
        "rule.auto_stop",
        Some("rule"),
        Some(rule_id),
        Some(&format!("reason={reason},dispatched={dispatched}")),
        true,
        None,
    )
    .await;
    tracing::info!(rule_id, node_id, reason, dispatched, "rule auto-stopped");

    Ok(())
}

/// 周期扫描已 expires_at < now 但仍 enabled 的规则,把它们 stop 掉。
/// report_rule_stats 内的 inline 检查只在 stats tick 上报时触发;
/// 若一条规则到期前后都没有流量(因此 Agent 不会发 stats batch),靠这个 sweep 兜底。
///
/// 周期 SWEEP_INTERVAL_SECS 默认 300s,可通过 env `PANEL_EXPIRY_SWEEP_SECS` 覆盖(测试用)。
pub fn spawn_expiry_sweeper(state: AppState) {
    use std::time::Duration;
    let secs: u64 = std::env::var("PANEL_EXPIRY_SWEEP_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
        .max(10);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match sqlx::query_as::<_, (i64,)>(
                "SELECT id FROM forward_rules \
                 WHERE enabled = 1 AND deleted_at IS NULL \
                   AND expires_at IS NOT NULL \
                   AND expires_at <= datetime('now')",
            )
            .fetch_all(&state.pool)
            .await
            {
                Ok(rows) => {
                    for (rule_id,) in rows {
                        if let Err(e) = auto_stop_if_exceeded(&state, rule_id).await {
                            warn!(error = ?e, rule_id, "expiry sweep auto_stop failed");
                        }
                    }
                }
                Err(e) => warn!(error = ?e, "expiry sweep query failed"),
            }
        }
    });
}

