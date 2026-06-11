//! 出站 webhook 通知(v1):fire-and-forget POST JSON。
//! 事件允许丢失——失败重试 1 次后放弃并 warn,绝不阻塞调用方。
//! URL 仅 admin 可配(system_settings.notify_webhook_url),不做内网地址过滤
//! (面板机本就由 admin 控制,SSRF 面已在 spec §C 记录)。
//!
//! 事件清单 v1:
//! - node.offline / node.online   data = { node_id, name? }
//! - user.expired / user.quota_exceeded   data = { user_id, disabled_rule_count }
use crate::state::AppState;
use tracing::warn;

/// 异步发送事件,调用处不 await 结果。
/// payload 形态: { "event": <str>, "occurred_at": <UTC RFC3339>, "data": <data> }
pub fn spawn_send(state: AppState, event: &'static str, data: serde_json::Value) {
    tokio::spawn(async move {
        let url: Option<String> = match sqlx::query_scalar::<_, String>(
            "SELECT value FROM system_settings WHERE key = 'notify_webhook_url'",
        )
        .fetch_optional(&state.pool)
        .await
        {
            Ok(v) => v.filter(|v| !v.is_empty()),
            Err(e) => {
                warn!(event, error = ?e, "webhook config read failed; event dropped");
                return;
            }
        };
        let Some(url) = url else { return };

        let payload = serde_json::json!({
            "event": event,
            "occurred_at": chrono::Utc::now().to_rfc3339(),
            "data": data,
        });
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!(error = ?e, "webhook client build failed");
                return;
            }
        };
        for attempt in 0..2u8 {
            match client.post(&url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => return,
                Ok(resp) => warn!(event, status = %resp.status(), attempt, "webhook non-2xx"),
                Err(e) => warn!(event, error = ?e, attempt, "webhook send failed"),
            }
        }
    });
}
