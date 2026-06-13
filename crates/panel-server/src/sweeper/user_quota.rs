//! 用户级到期 / 滚动 30 天流量配额 sweeper(P2)。
//! 取代已退役的规则级 expiry sweeper:一个 tokio task 内两个独立 interval,
//! expiry 默认 60s(PANEL_USER_EXPIRY_SWEEP_SECS),quota 默认 300s(PANEL_USER_QUOTA_SWEEP_SECS)。
use super::env_secs;
use crate::{audit, models::rule::Rule, state::AppState};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use std::time::Duration;
use tracing::{info, warn};

/// 计费字节聚合子表达式(滚动/月度两路径共用,避免重复)。
/// 隧道规则按 billing_mode/traffic_ratio 换算,原始 rule_stats 不变;mode=1 单向取较大
/// 方向(CASE 而非 SQLite 专有 max(a,b),保 PG 兼容)、=2 或非隧道取 rx+tx;倍率默认 1.0。
/// t.deleted_at IS NULL 在 ON 而非 WHERE:隧道软删后回落默认计费而非整行漏算。
/// CAST(REAL AS INTEGER):SQLite 向零截断、PG 四舍五入——小数倍率下两库差 ≤1 字节/聚合,
/// 不影响配额判定方向(不用 FLOOR/TRUNC:SQLite 数学函数需编译期开关,不保证可用)。
const BILLED_BYTES_SUM: &str = "COALESCE(CAST(SUM( \
    (CASE WHEN t.billing_mode = 1 \
          THEN CASE WHEN rs.rx_bytes > rs.tx_bytes THEN rs.rx_bytes ELSE rs.tx_bytes END \
          ELSE rs.rx_bytes + rs.tx_bytes END) \
    * COALESCE(t.traffic_ratio, 1.0) \
) AS INTEGER), 0)";

/// 月度模式本期起点:给定 now 与重置日 day(1-31),返回最近一次「day 日 00:00:00 UTC」
/// 的 "YYYY-MM-DD HH:MM:SS" 字符串(与 rule_stats.bucket_at 同格式)。
/// 月末容错:取 min(day, 当月天数);若本月该日尚未到则回上个月该日。纯函数,便于测试。
fn monthly_period_start(now: DateTime<Utc>, day: i64) -> String {
    let d = day.clamp(1, 31) as u32;
    let (y, m) = (now.year(), now.month());
    let candidate = clamp_day_start(y, m, d);
    let start = if candidate <= now {
        candidate
    } else {
        // 回上个月。
        let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        clamp_day_start(py, pm, d)
    };
    start.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 构造 year-month 的第 min(day, 当月天数) 日 00:00:00 UTC。
fn clamp_day_start(year: i32, month: u32, day: u32) -> DateTime<Utc> {
    let dim = days_in_month(year, month);
    let d = day.min(dim);
    Utc.with_ymd_and_hms(year, month, d, 0, 0, 0).unwrap()
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let first_next = Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).unwrap();
    let last = first_next - chrono::Duration::days(1);
    last.day()
}

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
    // 滚动 30 天模式(quota_reset_day IS NULL):一次 UPDATE 全量刷新。
    // 月度模式(quota_reset_day 非 NULL):窗口起点按用户重置日逐户计算(下方循环)。
    let rolling_sql = format!(
        "UPDATE users SET \
             period_used_bytes_cached = ( \
                 SELECT {billed} \
                 FROM rule_stats rs \
                 JOIN forward_rules fr ON rs.rule_id = fr.id \
                 LEFT JOIN tunnels t ON fr.tunnel_id = t.id AND t.deleted_at IS NULL \
                 WHERE fr.user_id = users.id \
                   AND rs.bucket_at >= datetime('now', '-30 days') \
             ), \
             period_used_calculated_at = datetime('now') \
         WHERE deleted_at IS NULL AND quota_reset_day IS NULL",
        billed = BILLED_BYTES_SUM,
    );
    sqlx::query(&rolling_sql).execute(&state.pool).await?;

    // 月度模式:逐户用 chrono 算本期起点(避免 SQL 日期算术的跨库不一致),
    // 再以绑定的 cutoff 刷新。月度用户通常是少数,逐户一次 UPDATE 可接受。
    let monthly: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, quota_reset_day FROM users \
         WHERE deleted_at IS NULL AND quota_reset_day IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await?;
    let now = chrono::Utc::now();
    let monthly_sql = format!(
        "UPDATE users SET \
             period_used_bytes_cached = ( \
                 SELECT {billed} \
                 FROM rule_stats rs \
                 JOIN forward_rules fr ON rs.rule_id = fr.id \
                 LEFT JOIN tunnels t ON fr.tunnel_id = t.id AND t.deleted_at IS NULL \
                 WHERE fr.user_id = ? \
                   AND rs.bucket_at >= ? \
             ), \
             period_used_calculated_at = datetime('now') \
         WHERE id = ? AND deleted_at IS NULL",
        billed = BILLED_BYTES_SUM,
    );
    for (uid, day) in monthly {
        let cutoff = monthly_period_start(now, day);
        sqlx::query(&monthly_sql)
            .bind(uid)
            .bind(&cutoff)
            .bind(uid)
            .execute(&state.pool)
            .await?;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn monthly_start_same_month_when_day_passed() {
        // 6/20,重置日 15 → 本月 6/15。
        assert_eq!(
            monthly_period_start(at("2026-06-20T10:00:00Z"), 15),
            "2026-06-15 00:00:00"
        );
    }

    #[test]
    fn monthly_start_previous_month_when_day_not_reached() {
        // 6/5,重置日 15 → 上月 5/15。
        assert_eq!(
            monthly_period_start(at("2026-06-05T10:00:00Z"), 15),
            "2026-05-15 00:00:00"
        );
    }

    #[test]
    fn monthly_start_on_reset_day_uses_today() {
        // 恰好重置日当天 00:00 之后 → 本月该日(边界 <=)。
        assert_eq!(
            monthly_period_start(at("2026-06-15T00:00:00Z"), 15),
            "2026-06-15 00:00:00"
        );
    }

    #[test]
    fn monthly_start_clamps_day_31_in_short_month() {
        // 重置日 31,4 月只有 30 天 → 4/30。4/29 时尚未到 4/30 → 回 3/31。
        assert_eq!(
            monthly_period_start(at("2026-04-30T12:00:00Z"), 31),
            "2026-04-30 00:00:00"
        );
        assert_eq!(
            monthly_period_start(at("2026-04-29T12:00:00Z"), 31),
            "2026-03-31 00:00:00"
        );
    }

    #[test]
    fn monthly_start_crosses_year_boundary() {
        // 1/5,重置日 15 → 上一年 12/15。
        assert_eq!(
            monthly_period_start(at("2026-01-05T10:00:00Z"), 15),
            "2025-12-15 00:00:00"
        );
    }

    #[test]
    fn monthly_start_feb_leap_clamp() {
        // 重置日 30,2024 闰年 2 月 29 天 → 2/29。
        assert_eq!(
            monthly_period_start(at("2024-03-01T00:00:00Z"), 30),
            "2024-02-29 00:00:00"
        );
    }
}
