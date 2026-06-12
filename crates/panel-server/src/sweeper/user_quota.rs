//! 用户级到期 / 滚动 30 天流量配额 sweeper(P2)。
//! 取代已退役的规则级 expiry sweeper:一个 tokio task 内两个独立 interval,
//! expiry 默认 60s(PANEL_USER_EXPIRY_SWEEP_SECS),quota 默认 300s(PANEL_USER_QUOTA_SWEEP_SECS)。
use super::env_secs;
use crate::{audit, models::rule::Rule, state::AppState};
use std::time::Duration;
use tracing::{info, warn};

pub fn spawn_user_quota_sweeper(state: AppState) {
    let expiry_secs = env_secs("PANEL_USER_EXPIRY_SWEEP_SECS", 60, 5);
    let quota_secs = env_secs("PANEL_USER_QUOTA_SWEEP_SECS", 300, 5);
    tokio::spawn(async move {
        let mut expiry_tick = tokio::time::interval(Duration::from_secs(expiry_secs));
        let mut quota_tick = tokio::time::interval(Duration::from_secs(quota_secs));
        expiry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        quota_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = expiry_tick.tick() => {
                    if let Err(e) = expiry_tick_once(&state).await {
                        warn!(error = ?e, "user expiry sweep failed");
                    }
                }
                _ = quota_tick.tick() => {
                    if let Err(e) = quota_tick_once(&state).await {
                        warn!(error = ?e, "user quota sweep failed");
                    }
                }
            }
        }
    });
}

/// 扫已过期用户,停其名下 enabled 规则。返回命中的用户数。
/// pub 供 integration tests 直接调用(确定性,不等 interval)。
pub async fn expiry_tick_once(state: &AppState) -> anyhow::Result<u64> {
    let users: Vec<(i64,)> = sqlx::query_as(
        "SELECT u.id FROM users u \
         WHERE u.expires_at IS NOT NULL AND u.expires_at <= datetime('now') \
           AND u.deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM forward_rules fr \
                       WHERE fr.user_id = u.id AND fr.enabled = 1 AND fr.deleted_at IS NULL)",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut hit = 0u64;
    for (user_id,) in users {
        let disabled = disable_rules_for_user(state, user_id, "expired").await?;
        if disabled > 0 {
            audit::record(
                &state.pool,
                None,
                "user.expired_auto_disable_rules",
                Some("user"),
                Some(user_id),
                Some(&format!("user_id={user_id},disabled_rule_count={disabled},reason=expired")),
                true,
                None,
            )
            .await;
            crate::notify::spawn_send(
                state.clone(),
                "user.expired",
                serde_json::json!({ "user_id": user_id, "disabled_rule_count": disabled }),
            );
            info!(user_id, disabled, "expired user rules auto-disabled");
            hit += 1;
        }
    }
    Ok(hit)
}

/// 先刷新所有活跃用户的 30 天用量 cache,再停超额用户的规则。返回命中的用户数。
/// 不变量:必须先刷 cache 再判定,禁止用过期 cache 判断超额。
pub async fn quota_tick_once(state: &AppState) -> anyhow::Result<u64> {
    // 注意:下面的 JOIN forward_rules 故意不过滤 fr.deleted_at——删除规则不得清零
    // 用户 30 天用量(防"删规则重建"规避配额),勿当 bug "修复"。
    //
    // 计费换算(P1):隧道规则按隧道 billing_mode/traffic_ratio 换算计费字节,原始
    // rule_stats 不变。billing_mode=1 单向取较大方向(CASE 而非 SQLite 专有 max(a,b),
    // 保 PG 兼容);=2 或非隧道规则取 rx+tx。倍率默认 1.0。
    // CAST(REAL AS INTEGER):SQLite 向零截断、PG 四舍五入——小数倍率下两库结果最多
    // 差 1 字节/聚合,不影响配额判定方向,可接受(不用 FLOOR/TRUNC:SQLite 数学函数
    // 需编译期 SQLITE_ENABLE_MATH_FUNCTIONS,不保证可用)。
    // t.deleted_at IS NULL 放 ON 而非 WHERE:隧道软删后该规则回落默认计费,而非被整行漏算。
    sqlx::query(
        "UPDATE users SET \
             period_used_bytes_cached = ( \
                 SELECT COALESCE(CAST(SUM( \
                     (CASE WHEN t.billing_mode = 1 \
                           THEN CASE WHEN rs.rx_bytes > rs.tx_bytes THEN rs.rx_bytes ELSE rs.tx_bytes END \
                           ELSE rs.rx_bytes + rs.tx_bytes END) \
                     * COALESCE(t.traffic_ratio, 1.0) \
                 ) AS INTEGER), 0) \
                 FROM rule_stats rs \
                 JOIN forward_rules fr ON rs.rule_id = fr.id \
                 LEFT JOIN tunnels t ON fr.tunnel_id = t.id AND t.deleted_at IS NULL \
                 WHERE fr.user_id = users.id \
                   AND rs.bucket_at >= datetime('now', '-30 days') \
             ), \
             period_used_calculated_at = datetime('now') \
         WHERE deleted_at IS NULL",
    )
    .execute(&state.pool)
    .await?;

    let users: Vec<(i64,)> = sqlx::query_as(
        "SELECT u.id FROM users u \
         WHERE u.traffic_limit_bytes_30d IS NOT NULL \
           AND u.period_used_bytes_cached > u.traffic_limit_bytes_30d \
           AND u.deleted_at IS NULL \
           AND EXISTS (SELECT 1 FROM forward_rules fr \
                       WHERE fr.user_id = u.id AND fr.enabled = 1 AND fr.deleted_at IS NULL)",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut hit = 0u64;
    for (user_id,) in users {
        let disabled = disable_rules_for_user(state, user_id, "quota_exceeded").await?;
        if disabled > 0 {
            audit::record(
                &state.pool,
                None,
                "user.quota_exceeded_auto_disable_rules",
                Some("user"),
                Some(user_id),
                Some(&format!(
                    "user_id={user_id},disabled_rule_count={disabled},reason=quota_exceeded"
                )),
                true,
                None,
            )
            .await;
            crate::notify::spawn_send(
                state.clone(),
                "user.quota_exceeded",
                serde_json::json!({ "user_id": user_id, "disabled_rule_count": disabled }),
            );
            info!(user_id, disabled, "over-quota user rules auto-disabled");
            hit += 1;
        }
    }
    Ok(hit)
}

/// 原子停用某用户全部 enabled 规则并逐条 dispatch ApplyRule(enabled=false)。
/// 返回实际停掉的行数。Agent 离线时静默(下次 register reconcile 对齐)。
async fn disable_rules_for_user(state: &AppState, user_id: i64, _reason: &str) -> anyhow::Result<u64> {
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM forward_rules WHERE user_id = ? AND enabled = 1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    if ids.is_empty() {
        return Ok(0);
    }
    // 单次 UPDATE 原子落库,但只停上面捕获到的 id:SELECT 与 UPDATE 之间新建的规则
    // 不能被按 user_id 整批误停(否则 dispatch 循环漏发且下个 tick 因 enabled=0 不再命中);
    // 留给下个 tick 的 EXISTS 重新捕获。WHERE enabled = 1 防并发重复触发。
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "UPDATE forward_rules SET enabled = 0, updated_at = datetime('now') \
         WHERE id IN ({placeholders}) AND enabled = 1 AND deleted_at IS NULL"
    );
    let mut q = sqlx::query(&sql);
    for (id,) in &ids {
        q = q.bind(id);
    }
    let rows = q.execute(&state.pool).await?.rows_affected();
    for (rule_id,) in ids {
        match Rule::find_by_id(&state.pool, rule_id).await {
            Ok(Some(rule)) => {
                let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(state, &rule).await;
            }
            // 窗口内被软删:DB 已是终态,无需下发。
            Ok(None) => {}
            Err(e) => warn!(error = ?e, rule_id, "find rule for disable dispatch failed"),
        }
    }
    Ok(rows)
}
