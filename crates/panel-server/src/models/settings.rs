use sqlx::SqlitePool;

/// 读取 reserved_ports 配置。失败一律返回空列表（端口禁用列表失效不应阻断主流程，
/// 但应通过 audit_logs / tracing 暴露）。
pub async fn reserved_ports(pool: &SqlitePool) -> Vec<i64> {
    let value: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT value FROM system_settings WHERE key = 'reserved_ports'",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    let Some(value) = value else { return Vec::new() };
    serde_json::from_str::<Vec<i64>>(&value).unwrap_or_default()
}
