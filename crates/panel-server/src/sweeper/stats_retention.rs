//! rule_stats / node_stats 按 system_settings.stats_retention_days 滚动清理。
//! 不清理 audit_logs(审计保留是合规属性,不在本任务语义内)。
use crate::state::AppState;
use std::time::Duration;
use tracing::{info, warn};

const BATCH: i64 = 5000;

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .max(5)
}

pub fn spawn_stats_retention_sweeper(state: AppState) {
    let secs = env_secs("PANEL_STATS_RETENTION_SWEEP_SECS", 3600);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(e) = retention_tick_once(&state).await {
                warn!(error = ?e, "stats retention sweep failed");
            }
        }
    });
}

/// 每 tick 现读配置(改完即生效);缺省/非法值回落 30 天。返回总删除行数。
/// pub 供集成测试直接调用(确定性,不等 interval)。
pub async fn retention_tick_once(state: &AppState) -> anyhow::Result<u64> {
    // DB 读取失败必须传播(跳过本 tick),不得在错误态下按默认 30 天做不可逆删除;
    // 「缺失/非法值回落 30 天」仅适用于读取成功的场景。
    let days: i64 = sqlx::query_scalar::<_, String>(
        "SELECT value FROM system_settings WHERE key = 'stats_retention_days'",
    )
    .fetch_optional(&state.pool)
    .await?
    .and_then(|v| v.parse().ok())
    .filter(|n| *n >= 1)
    .unwrap_or(30);
    let cutoff_modifier = format!("-{days} days");

    let mut total = 0u64;
    for table in ["rule_stats", "node_stats"] {
        loop {
            // 子查询 + id IN:PG 兼容(不依赖 SQLite rowid / DELETE LIMIT 编译开关);
            // 分批防长事务锁库。表名来自上面的常量数组,无注入面。
            let sql = format!(
                "DELETE FROM {table} WHERE id IN \
                 (SELECT id FROM {table} WHERE bucket_at < datetime('now', ?) LIMIT {BATCH})"
            );
            let rows = sqlx::query(&sql)
                .bind(&cutoff_modifier)
                .execute(&state.pool)
                .await?
                .rows_affected();
            total += rows;
            if rows < BATCH as u64 {
                break;
            }
        }
    }
    if total > 0 {
        info!(deleted = total, retention_days = days, "stats retention sweep deleted old buckets");
    }
    Ok(total)
}
