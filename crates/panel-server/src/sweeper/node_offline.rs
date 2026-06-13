//! 节点掉线检测:status='online' 且 last_seen_at 超过阈值 → 置 offline。
//! 修复「节点永不掉线」bug(此前没有任何代码把 status 写回 offline),
//! 并作为 node.offline webhook 事件源。
//! 恢复(offline→online)在 grpc/service.rs 写 online 的三处检测并发 node.online。
use super::env_secs;
use crate::{audit, notify, state::AppState};
use std::time::Duration;
use tracing::{info, warn};

pub fn spawn_node_offline_sweeper(state: AppState) {
    let secs = env_secs("PANEL_NODE_OFFLINE_SWEEP_SECS", 30, 5);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(e) = offline_tick_once(&state).await {
                warn!(error = ?e, "node offline sweep failed");
            }
        }
    });
}

/// 返回本次翻转 offline 的节点数。pub 供集成测试直接调用(确定性,不等 interval)。
pub async fn offline_tick_once(state: &AppState) -> anyhow::Result<u64> {
    // 默认 120s = 2 个 Agent 心跳周期(60s),容忍一次心跳丢失;下限 10s 防误配抖动。
    let after = env_secs("PANEL_NODE_OFFLINE_AFTER_SECS", 120, 10) as i64;
    let modifier = format!("-{after} seconds");

    let stale: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM nodes \
         WHERE status = 'online' AND deleted_at IS NULL \
           AND (last_seen_at IS NULL OR last_seen_at < datetime('now', ?))",
    )
    .bind(&modifier)
    .fetch_all(&state.pool)
    .await?;
    if stale.is_empty() {
        return Ok(0);
    }

    // 置 offline 时再次校验条件,防 SELECT→UPDATE 窗口内心跳进来被误翻。
    let mut flipped = 0u64;
    let mut names = Vec::new();
    for (id, name) in &stale {
        let rows = sqlx::query(
            "UPDATE nodes SET status = 'offline', updated_at = datetime('now') \
             WHERE id = ? AND status = 'online' \
               AND (last_seen_at IS NULL OR last_seen_at < datetime('now', ?))",
        )
        .bind(id)
        .bind(&modifier)
        .execute(&state.pool)
        .await?
        .rows_affected();
        if rows > 0 {
            flipped += 1;
            names.push(format!("#{id}({name})"));
            notify::spawn_send(
                state.clone(),
                "node.offline",
                serde_json::json!({ "node_id": id, "name": name }),
            );
            // SSE:掉线 → 推送变更。
            state.publish_node_event(*id);
        }
    }
    if flipped > 0 {
        audit::record(
            &state.pool,
            None,
            "node.offline_detected",
            Some("node"),
            None,
            Some(&format!("count={flipped},nodes={}", names.join(","))),
            true,
            None,
        )
        .await;
        info!(count = flipped, "nodes marked offline by sweeper");
    }
    Ok(flipped)
}
