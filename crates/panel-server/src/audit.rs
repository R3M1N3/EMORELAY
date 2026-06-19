use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::warn;

/// 写一条审计日志。失败不应阻止主流程,因此吞掉错误并 warn。
/// 无 IP 上下文(后台任务、内部 RPC)用此版本,actor_ip 落 NULL。
pub async fn record(
    pool: &SqlitePool,
    actor_user_id: Option<i64>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<i64>,
    payload: Option<&str>,
    success: bool,
    error_message: Option<&str>,
) {
    record_with_ip(
        pool,
        actor_user_id,
        None,
        action,
        target_type,
        target_id,
        payload,
        success,
        error_message,
    )
    .await
}

/// HTTP handler 用此版本,把 ActorIp extractor 拿到的 IP 透传过来。
#[allow(clippy::too_many_arguments)]
pub async fn record_with_ip(
    pool: &SqlitePool,
    actor_user_id: Option<i64>,
    actor_ip: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<i64>,
    payload: Option<&str>,
    success: bool,
    error_message: Option<&str>,
) {
    let result_str = if success { "success" } else { "failure" };
    let outcome = sqlx::query(
        "INSERT INTO audit_logs \
            (actor_user_id, actor_ip, action, target_type, target_id, payload, result, error_message) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(actor_user_id)
    .bind(actor_ip)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(payload)
    .bind(result_str)
    .bind(error_message)
    .execute(pool)
    .await;
    if let Err(e) = outcome {
        warn!(error = ?e, action = %action, "failed to write audit log");
    }
}

/// 失败登录审计的去重节流窗口:同一来源 IP 在此窗口内只落一条 auth.login 失败审计,
/// 其余失败仅累加内存计数,窗口翻转时把累计数附在新一条审计的 error_message 上。
const LOGIN_AUDIT_WINDOW: Duration = Duration::from_secs(60);

/// [`LoginAuditThrottle::decide`] 的判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum AuditDecision {
    /// 应写库;`prev_suppressed` = 上一窗口被抑制(未单独记录)的失败次数,>0 时附到 error_message。
    Record { prev_suppressed: u32 },
    /// 窗口内重复失败,抑制写库(仅累加计数)。
    Suppress,
}

struct Window {
    start: Instant,
    suppressed: u32,
}

/// 进程内的失败登录审计节流器(线程安全,随 `AppState` 共享)。
///
/// 目的:防止(尤其分布式)密码爆破把 `audit_logs` 表与设置页「最近 N 条」视图刷满,淹没有用记录。
/// 它**只**决定"失败登录是否写进审计表",不影响登录鉴权结果,也与 per-IP 登录限速层(governor)互补。
#[derive(Default)]
pub struct LoginAuditThrottle {
    inner: Mutex<HashMap<String, Window>>,
}

impl LoginAuditThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// 判定某来源 `key`(通常是客户端 IP)此刻的失败登录是否应写审计。
    /// `now` 作参数传入便于单测(生产传 `Instant::now()`)。
    pub fn decide(&self, key: &str, now: Instant) -> AuditDecision {
        let mut map = self.inner.lock().unwrap();
        let decision = match map.get_mut(key) {
            // 窗口内重复失败:抑制,累加计数。
            Some(w) if now.saturating_duration_since(w.start) < LOGIN_AUDIT_WINDOW => {
                w.suppressed = w.suppressed.saturating_add(1);
                AuditDecision::Suppress
            }
            // 已有记录但窗口已过期:开新窗口,报告上一窗口的抑制数。
            Some(w) => {
                let prev = w.suppressed;
                w.start = now;
                w.suppressed = 0;
                AuditDecision::Record {
                    prev_suppressed: prev,
                }
            }
            // 首次出现:写库,开窗口。
            None => {
                map.insert(
                    key.to_string(),
                    Window {
                        start: now,
                        suppressed: 0,
                    },
                );
                AuditDecision::Record { prev_suppressed: 0 }
            }
        };
        // 清理过期项防无界增长(保留当前 key,及未过 2×窗口的项,给跨窗口报告留机会)。
        map.retain(|k, w| {
            k == key || now.saturating_duration_since(w.start) < LOGIN_AUDIT_WINDOW.saturating_mul(2)
        });
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_records_first_then_suppresses_within_window() {
        let t = LoginAuditThrottle::new();
        let t0 = Instant::now();
        assert_eq!(
            t.decide("1.2.3.4", t0),
            AuditDecision::Record { prev_suppressed: 0 }
        );
        assert_eq!(
            t.decide("1.2.3.4", t0 + Duration::from_secs(1)),
            AuditDecision::Suppress
        );
        assert_eq!(
            t.decide("1.2.3.4", t0 + Duration::from_secs(30)),
            AuditDecision::Suppress
        );
    }

    #[test]
    fn throttle_isolates_per_key() {
        let t = LoginAuditThrottle::new();
        let t0 = Instant::now();
        assert_eq!(
            t.decide("1.1.1.1", t0),
            AuditDecision::Record { prev_suppressed: 0 }
        );
        // 不同 IP 各自独立,首次都写库。
        assert_eq!(
            t.decide("2.2.2.2", t0 + Duration::from_secs(1)),
            AuditDecision::Record { prev_suppressed: 0 }
        );
    }

    #[test]
    fn throttle_reports_suppressed_count_on_window_rollover() {
        let t = LoginAuditThrottle::new();
        let t0 = Instant::now();
        assert_eq!(
            t.decide("9.9.9.9", t0),
            AuditDecision::Record { prev_suppressed: 0 }
        );
        for i in 1..=3 {
            assert_eq!(
                t.decide("9.9.9.9", t0 + Duration::from_secs(i)),
                AuditDecision::Suppress
            );
        }
        // 窗口过期(>60s):开新窗口并报告上一窗口抑制了 3 次。
        assert_eq!(
            t.decide("9.9.9.9", t0 + Duration::from_secs(61)),
            AuditDecision::Record { prev_suppressed: 3 }
        );
        // 新窗口内重新从 0 计。
        assert_eq!(
            t.decide("9.9.9.9", t0 + Duration::from_secs(62)),
            AuditDecision::Suppress
        );
    }
}
