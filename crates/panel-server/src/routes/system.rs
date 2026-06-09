use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use std::collections::HashMap;

// ============= overview =============

#[derive(Serialize)]
pub struct SystemOverview {
    pub total_nodes: i64,
    pub online_nodes: i64,
    pub total_rules: i64,
    pub enabled_rules: i64,
    pub rx_bytes_total: i64,
    pub tx_bytes_total: i64,
}

pub async fn overview(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<SystemOverview>> {
    auth.require_admin()?;

    let (total_nodes, online_nodes, rx, tx): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*), \
            COALESCE(SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END), 0), \
            COALESCE(SUM(rx_bytes_total), 0), \
            COALESCE(SUM(tx_bytes_total), 0) \
         FROM nodes WHERE deleted_at IS NULL",
    )
    .fetch_one(&state.pool)
    .await?;

    let (total_rules, enabled_rules): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(enabled), 0) \
         FROM forward_rules WHERE deleted_at IS NULL",
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(SystemOverview {
        total_nodes,
        online_nodes,
        total_rules,
        enabled_rules,
        rx_bytes_total: rx,
        tx_bytes_total: tx,
    }))
}

// ============= security info =============

#[derive(Serialize)]
pub struct SecurityInfo {
    /// JWT secret 是否已从环境变量加载(Config::from_env 缺失会直接 fail-fast,所以这里一般为 true)。
    pub jwt_secret_configured: bool,
    /// secret 字节长度(String::len),仅供管理员肉眼判断强度,不暴露内容。
    pub jwt_secret_length: usize,
    pub jwt_expiry_hours: u64,
    /// gRPC 控制面是否启用 TLS。false 时 Agent 与 Server 间 token 明文。
    pub grpc_tls_enabled: bool,
    /// 是否启用 mTLS (Server 校验 client cert + Agent 同时启用 cert 才生效)。
    /// 仅反映 Server 端 PANEL_GRPC_TLS_CLIENT_CA 是否配置,Agent 端是否带 client cert 不在此判断。
    pub grpc_mtls_enabled: bool,
}

pub async fn security(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<SecurityInfo>> {
    auth.require_admin()?;
    let cfg = &state.config;
    let tls_on = cfg.grpc_tls_cert.is_some() && cfg.grpc_tls_key.is_some();
    Ok(Json(SecurityInfo {
        jwt_secret_configured: !cfg.jwt_secret.is_empty(),
        jwt_secret_length: cfg.jwt_secret.len(),
        jwt_expiry_hours: cfg.jwt_expiry_hours,
        grpc_tls_enabled: tls_on,
        grpc_mtls_enabled: tls_on && cfg.grpc_tls_client_ca.is_some(),
    }))
}

// ============= audit logs =============

#[derive(Deserialize)]
pub struct AuditLogQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub action: Option<String>,
    pub target_type: Option<String>,
    pub result: Option<String>,
}

#[derive(Serialize, FromRow)]
pub struct AuditLogEntry {
    pub id: i64,
    pub actor_user_id: Option<i64>,
    pub actor_ip: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<i64>,
    pub payload: Option<String>,
    pub result: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct AuditLogListResponse {
    pub items: Vec<AuditLogEntry>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

pub async fn audit_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<AuditLogQuery>,
) -> ApiResult<Json<AuditLogListResponse>> {
    auth.require_admin()?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(50).clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(page_size);

    if let Some(r) = q.result.as_deref() {
        if !matches!(r, "success" | "failure") {
            return Err(ApiError::BadRequest(
                "result must be success or failure".into(),
            ));
        }
    }

    // 拼字符串只拼 WHERE 条件名,值都走 bind,无 SQL 注入风险。
    let mut where_parts: Vec<&str> = vec!["1=1"];
    if q.action.is_some() {
        where_parts.push("action = ?");
    }
    if q.target_type.is_some() {
        where_parts.push("target_type = ?");
    }
    if q.result.is_some() {
        where_parts.push("result = ?");
    }
    let where_clause = where_parts.join(" AND ");
    let select_sql = format!(
        "SELECT id, actor_user_id, actor_ip, action, target_type, target_id, payload, \
                result, error_message, created_at \
         FROM audit_logs WHERE {where_clause} ORDER BY id DESC LIMIT ? OFFSET ?"
    );
    let count_sql = format!("SELECT COUNT(*) FROM audit_logs WHERE {where_clause}");

    let mut select_q = sqlx::query_as::<_, AuditLogEntry>(&select_sql);
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(a) = &q.action {
        select_q = select_q.bind(a);
        count_q = count_q.bind(a);
    }
    if let Some(t) = &q.target_type {
        select_q = select_q.bind(t);
        count_q = count_q.bind(t);
    }
    if let Some(r) = &q.result {
        select_q = select_q.bind(r);
        count_q = count_q.bind(r);
    }
    let items = select_q
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
    let total = count_q.fetch_one(&state.pool).await?;

    Ok(Json(AuditLogListResponse {
        items,
        total,
        page,
        page_size,
    }))
}

// ============= settings =============

#[derive(Serialize)]
pub struct SettingsResponse {
    pub settings: HashMap<String, String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<SettingsResponse>> {
    auth.require_admin()?;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM system_settings ORDER BY key")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(SettingsResponse {
        settings: rows.into_iter().collect(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateSettingsRequest {
    pub settings: HashMap<String, String>,
}

pub async fn update_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<UpdateSettingsRequest>,
) -> ApiResult<Json<SettingsResponse>> {
    auth.require_admin()?;

    // 已知 key 白名单。未知 key 拒绝(防误植入新 K/V)。
    const ALLOWED: &[&str] = &[
        "reserved_ports",
        "default_traffic_limit_bytes",
        "default_bandwidth_limit_mbps",
        "stats_retention_days",
    ];

    for (k, v) in req.settings.iter() {
        if !ALLOWED.contains(&k.as_str()) {
            return Err(ApiError::BadRequest(format!("unknown setting key: {k}")));
        }
        validate_setting(k, v)?;
    }

    let mut tx = state.pool.begin().await?;
    for (k, v) in req.settings.iter() {
        sqlx::query(
            "INSERT INTO system_settings (key, value, updated_at) \
             VALUES (?, ?, datetime('now')) \
             ON CONFLICT(key) DO UPDATE SET \
                value = excluded.value, \
                updated_at = datetime('now')",
        )
        .bind(k)
        .bind(v)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let mut keys: Vec<&String> = req.settings.keys().collect();
    keys.sort();
    let key_list = keys
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "system.update_settings",
        Some("system"),
        None,
        Some(&format!("keys={key_list}")),
        true,
        None,
    )
    .await;

    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM system_settings ORDER BY key")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(SettingsResponse {
        settings: rows.into_iter().collect(),
    }))
}

fn validate_setting(key: &str, value: &str) -> ApiResult<()> {
    match key {
        "reserved_ports" => {
            let ports: Vec<i64> = serde_json::from_str(value).map_err(|e| {
                ApiError::BadRequest(format!("reserved_ports must be JSON int array: {e}"))
            })?;
            for p in &ports {
                if !(1..=65535).contains(p) {
                    return Err(ApiError::BadRequest(format!(
                        "reserved_ports element {p} out of range 1-65535"
                    )));
                }
            }
            Ok(())
        }
        "default_traffic_limit_bytes" | "default_bandwidth_limit_mbps" => {
            if value.is_empty() {
                return Ok(());
            }
            let n: i64 = value.parse().map_err(|e| {
                ApiError::BadRequest(format!("{key} must be a non-negative integer: {e}"))
            })?;
            if n < 0 {
                return Err(ApiError::BadRequest(format!("{key} must be >= 0")));
            }
            Ok(())
        }
        "stats_retention_days" => {
            let n: i64 = value.parse().map_err(|e| {
                ApiError::BadRequest(format!("stats_retention_days must be positive integer: {e}"))
            })?;
            if n < 1 {
                return Err(ApiError::BadRequest(
                    "stats_retention_days must be >= 1".into(),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
