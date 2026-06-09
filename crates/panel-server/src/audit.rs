use sqlx::SqlitePool;
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
