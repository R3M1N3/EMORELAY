use sqlx::SqlitePool;

/// 读取 reserved_ports 配置。失败一律返回空列表（端口禁用列表失效不应阻断主流程，
/// 但应通过 audit_logs / tracing 暴露）。
pub async fn reserved_ports(pool: &SqlitePool) -> Vec<i64> {
    let value: Option<String> = match sqlx::query_scalar::<_, String>(
        "SELECT value FROM system_settings WHERE key = 'reserved_ports'",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // 安全降级必须可见:读失败=保留端口黑名单临时失效。
            tracing::error!(error = ?e, "读取 reserved_ports 失败,保留端口校验本次降级为空");
            return Vec::new();
        }
    };
    let Some(value) = value else { return Vec::new() };
    match serde_json::from_str::<Vec<i64>>(&value) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = ?e, value, "reserved_ports 解析失败,保留端口校验本次降级为空");
            Vec::new()
        }
    }
}
