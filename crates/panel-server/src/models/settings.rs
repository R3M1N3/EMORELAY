use sqlx::SqlitePool;

/// 配置缺失/损坏/读失败时的兜底保留端口集——与 migration 0001 种入的默认值一致。
/// fail-closed 原则:宁可多拦,也不让 22/80/443/3306/5432 在配置异常时被规则监听。
const DEFAULT_RESERVED_PORTS: [i64; 5] = [22, 80, 443, 3306, 5432];

/// 读取 reserved_ports 配置。**fail-closed**:DB 读失败 / 值缺失 / 解析失败时,
/// 一律回退到硬编码默认保留集(而非空集),避免配置异常导致保留端口黑名单静默失效。
/// 仅当配置存在且为合法 JSON(含管理员显式设置的空数组 `[]`)时才采用其值。
pub async fn reserved_ports(pool: &SqlitePool) -> Vec<i64> {
    let value: Option<String> = match sqlx::query_scalar::<_, String>(
        "SELECT value FROM system_settings WHERE key = 'reserved_ports'",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            // fail-closed:读失败回退默认保留集,黑名单不因 DB 抖动而失效。
            tracing::error!(error = ?e, "读取 reserved_ports 失败,回退默认保留端口集");
            return DEFAULT_RESERVED_PORTS.to_vec();
        }
    };
    // 值缺失(正常路径不会发生:migration 种入)→ 回退默认集,而非放空。
    let Some(value) = value else {
        return DEFAULT_RESERVED_PORTS.to_vec();
    };
    match serde_json::from_str::<Vec<i64>>(&value) {
        Ok(v) => v,
        Err(e) => {
            // fail-closed:值损坏(非法 JSON)回退默认保留集,而非放空。
            tracing::error!(error = ?e, value, "reserved_ports 解析失败,回退默认保留端口集");
            DEFAULT_RESERVED_PORTS.to_vec()
        }
    }
}
