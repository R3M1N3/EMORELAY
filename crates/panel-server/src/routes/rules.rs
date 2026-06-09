use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    grpc::commands::{apply_command, remove_command, restart_command},
    models::{
        node::Node,
        rule::{Rule, SORT_FIELDS},
        settings,
    },
    state::AppState,
    util::{is_valid_ip, is_valid_target_host},
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::prelude::FromRow;

// ============= DTOs =============

#[derive(Serialize)]
pub struct RuleView {
    pub id: i64,
    pub user_id: i64,
    pub node_id: i64,
    pub name: String,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: i64,
    pub target_host: String,
    pub target_port: i64,
    pub enabled: bool,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub connection_count: i64,
    pub bandwidth_profile_id: Option<i64>,
    pub bandwidth_mbps: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Rule> for RuleView {
    fn from(r: Rule) -> Self {
        Self {
            id: r.id,
            user_id: r.user_id,
            node_id: r.node_id,
            name: r.name,
            protocol: r.protocol,
            listen_ip: r.listen_ip,
            listen_port: r.listen_port,
            target_host: r.target_host,
            target_port: r.target_port,
            enabled: r.enabled != 0,
            rx_bytes: r.rx_bytes,
            tx_bytes: r.tx_bytes,
            connection_count: r.connection_count,
            bandwidth_profile_id: r.bandwidth_profile_id,
            bandwidth_mbps: r.bandwidth_mbps,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub node_id: Option<i64>,
    pub protocol: Option<String>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub struct RuleListResponse {
    pub items: Vec<RuleView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateRuleRequest {
    pub node_id: i64,
    pub name: String,
    pub protocol: String,
    #[serde(default = "default_listen_ip")]
    pub listen_ip: String,
    pub listen_port: Option<u16>,
    pub target_host: String,
    pub target_port: u16,
    pub bandwidth_profile_id: Option<i64>,
}

fn default_listen_ip() -> String {
    "0.0.0.0".to_string()
}

#[derive(Deserialize)]
pub struct UpdateRuleRequest {
    pub name: Option<String>,
    pub listen_ip: Option<String>,
    pub listen_port: Option<u16>,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
    /// 0 = 解除关联
    pub bandwidth_profile_id: Option<i64>,
}

#[derive(Serialize, FromRow)]
pub struct RuleStatsBucket {
    pub bucket_at: String,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub connection_count: i64,
    pub error_count: i64,
}

#[derive(Serialize)]
pub struct RuleStatsCurrent {
    pub enabled: bool,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub connection_count: i64,
}

#[derive(Serialize)]
pub struct RuleStatsResponse {
    pub current: RuleStatsCurrent,
    /// rule_stats 由单元 L Agent 上报后填充；本阶段返回空列表是预期行为。
    pub series: Vec<RuleStatsBucket>,
}

#[derive(Serialize, FromRow)]
pub struct RuleLogEntry {
    pub id: i64,
    pub actor_user_id: Option<i64>,
    pub action: String,
    pub result: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

// ============= handlers =============

/// 普通用户只能看 / 改自己 user_id 名下的规则;admin 不受限。
/// 失败一律 NotFound(避免通过 403 泄漏规则是否存在)。
fn ensure_can_touch(auth: &AuthUser, rule: &Rule) -> ApiResult<()> {
    if auth.is_admin() || rule.user_id == auth.0.sub {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<RuleListResponse>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);

    let sort_field = q.sort.as_deref().unwrap_or("id");
    if !SORT_FIELDS.contains(&sort_field) {
        return Err(ApiError::BadRequest(format!(
            "invalid sort field; allowed: {}",
            SORT_FIELDS.join(",")
        )));
    }
    let order_desc = match q.order.as_deref().unwrap_or("desc") {
        "asc" => false,
        "desc" => true,
        _ => return Err(ApiError::BadRequest("order must be asc or desc".into())),
    };
    if let Some(p) = q.protocol.as_deref() {
        validate_protocol(p)?;
    }

    // admin → None(不限);user → Some(自己 id)。
    let restrict_user_id = if auth.is_admin() { None } else { Some(auth.0.sub) };

    let items = Rule::list_paged(
        &state.pool,
        sort_field,
        order_desc,
        page_size,
        offset,
        q.node_id,
        q.protocol.as_deref(),
        q.search.as_deref(),
        restrict_user_id,
    )
    .await?;
    let total = Rule::count_filtered(
        &state.pool,
        q.node_id,
        q.protocol.as_deref(),
        q.search.as_deref(),
        restrict_user_id,
    )
    .await?;

    Ok(Json(RuleListResponse {
        items: items.into_iter().map(Into::into).collect(),
        total,
        page,
        page_size,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<RuleView>> {
    let rule = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &rule)?;
    Ok(Json(rule.into()))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateRuleRequest>,
) -> ApiResult<Json<RuleView>> {
    // 普通用户可以为自己创建规则;rule.user_id 设为 claims.sub。
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    validate_protocol(&req.protocol)?;
    if matches!(req.listen_port, Some(0)) || req.target_port == 0 {
        return Err(ApiError::BadRequest(
            "listen_port and target_port must be 1-65535".into(),
        ));
    }
    if !is_valid_ip(&req.listen_ip) {
        return Err(ApiError::BadRequest("listen_ip is not a valid IP".into()));
    }
    if !is_valid_target_host(&req.target_host) {
        return Err(ApiError::BadRequest(
            "target_host is not a valid IP or hostname".into(),
        ));
    }
    let node = Node::find_by_id(&state.pool, req.node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("node_id does not exist".into()))?;
    if let Some(pid) = req.bandwidth_profile_id {
        if pid <= 0 {
            return Err(ApiError::BadRequest("bandwidth_profile_id must be > 0".into()));
        }
        crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
            .await?
            .ok_or_else(|| ApiError::BadRequest("bandwidth_profile_id does not exist".into()))?;
    }
    let reserved = settings::reserved_ports(&state.pool).await;
    let listen_port_i64 = match req.listen_port {
        Some(p) => {
            let p = i64::from(p);
            if p < node.port_pool_min || p > node.port_pool_max {
                return Err(ApiError::BadRequest(format!(
                    "listen_port {} outside node's port pool [{}-{}]",
                    p, node.port_pool_min, node.port_pool_max
                )));
            }
            if reserved.contains(&p) {
                return Err(ApiError::BadRequest(format!("listen_port {p} is reserved")));
            }
            p
        }
        // 留空 → 池内最小可用端口(排除 reserved 与按协议互斥的占用)。
        None => allocate_port(&state.pool, &node, &req.listen_ip, &req.protocol, &reserved).await?,
    };

    // UNIQUE 索引按 protocol 字符串精确匹配,tcp_udp 与 tcp/udp 在 DB 层不互斥;此处补应用层互斥。
    ensure_no_protocol_conflict(
        &state.pool,
        req.node_id,
        &req.listen_ip,
        listen_port_i64,
        &req.protocol,
        None,
    )
    .await?;

    let new_id = Rule::create(
        &state.pool,
        auth.0.sub,
        req.node_id,
        name,
        &req.protocol,
        &req.listen_ip,
        listen_port_i64,
        req.target_host.trim(),
        i64::from(req.target_port),
        req.bandwidth_profile_id,
    )
    .await
    .map_err(map_sqlx_to_api)?;

    let rule = Rule::find_by_id(&state.pool, new_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "rule.create",
        Some("rule"),
        Some(new_id),
        Some(name),
        true,
        None,
    )
    .await;

    if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
        tracing::warn!(node_id = rule.node_id, rule_id = rule.id, "agent offline; rule will sync at next register");
    }

    Ok(Json(rule.into()))
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRuleRequest>,
) -> ApiResult<Json<RuleView>> {
    let existing = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &existing)?;

    if let Some(n) = req.name.as_deref() {
        if n.trim().is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
    }
    if let Some(ip) = req.listen_ip.as_deref() {
        if !is_valid_ip(ip) {
            return Err(ApiError::BadRequest("listen_ip is not a valid IP".into()));
        }
    }
    if let Some(host) = req.target_host.as_deref() {
        if !is_valid_target_host(host.trim()) {
            return Err(ApiError::BadRequest(
                "target_host is not a valid IP or hostname".into(),
            ));
        }
    }
    if matches!(req.listen_port, Some(0)) || matches!(req.target_port, Some(0)) {
        return Err(ApiError::BadRequest("ports must be 1-65535".into()));
    }
    if let Some(pid) = req.bandwidth_profile_id {
        if pid < 0 {
            return Err(ApiError::BadRequest("bandwidth_profile_id must be >= 0".into()));
        }
        if pid > 0 {
            crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
                .await?
                .ok_or_else(|| ApiError::BadRequest("bandwidth_profile_id does not exist".into()))?;
        }
    }

    // 端口落入 node port_pool + reserved 校验。
    let effective_port = req.listen_port.map(i64::from).unwrap_or(existing.listen_port);
    let effective_ip = req.listen_ip.as_deref().unwrap_or(&existing.listen_ip);
    let node = Node::find_by_id(&state.pool, existing.node_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if effective_port < node.port_pool_min || effective_port > node.port_pool_max {
        return Err(ApiError::BadRequest(format!(
            "listen_port {} outside node's port pool [{}-{}]",
            effective_port, node.port_pool_min, node.port_pool_max
        )));
    }
    let reserved = settings::reserved_ports(&state.pool).await;
    if reserved.contains(&effective_port) {
        return Err(ApiError::BadRequest(format!(
            "listen_port {effective_port} is reserved"
        )));
    }

    // protocol 不可改,沿用 existing.protocol;listen_ip/port 改动需重新做互斥预检并排除自身。
    ensure_no_protocol_conflict(
        &state.pool,
        existing.node_id,
        effective_ip,
        effective_port,
        &existing.protocol,
        Some(id),
    )
    .await?;

    let rows = Rule::update_fields(
        &state.pool,
        id,
        req.name.as_deref().map(str::trim),
        req.listen_ip.as_deref(),
        req.listen_port.map(i64::from),
        req.target_host.as_deref().map(str::trim),
        req.target_port.map(i64::from),
        req.bandwidth_profile_id,
    )
    .await
    .map_err(map_sqlx_to_api)?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    let rule = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "rule.update",
        Some("rule"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
        tracing::warn!(node_id = rule.node_id, rule_id = rule.id, "agent offline; update will sync at next register");
    }

    Ok(Json(rule.into()))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    // 先取 node_id 才能定向下发 RemoveRule；软删后规则不可见。
    let existing = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &existing)?;
    let node_id = existing.node_id;

    let rows = Rule::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "rule.delete",
        Some("rule"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    if !state.dispatcher.dispatch(node_id, remove_command(id)) {
        tracing::warn!(node_id, rule_id = id, "agent offline; rule will be removed on next register reconcile");
    }

    Ok(Json(json!({ "ok": true })))
}

pub async fn enable(
    state: State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    path: Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    set_enabled_handler(state, auth, actor_ip, path, true, "rule.enable").await
}

pub async fn disable(
    state: State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    path: Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    set_enabled_handler(state, auth, actor_ip, path, false, "rule.disable").await
}

async fn set_enabled_handler(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    enabled: bool,
    action: &'static str,
) -> ApiResult<Json<serde_json::Value>> {
    // 取出规则做 owner 校验,再 set_enabled。
    let existing = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &existing)?;
    let rows = Rule::set_enabled(&state.pool, id, enabled).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        action,
        Some("rule"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    // 通过 ApplyRule 让 Agent 用新的 enabled 字段重新对齐（启停 listener）。
    if let Ok(Some(rule)) = Rule::find_by_id(&state.pool, id).await {
        if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
            tracing::warn!(node_id = rule.node_id, rule_id = id, action, "agent offline");
        }
    }

    Ok(Json(json!({ "ok": true, "enabled": enabled })))
}

pub async fn restart(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let rule = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &rule)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "rule.restart",
        Some("rule"),
        Some(id),
        None,
        true,
        None,
    )
    .await;
    let dispatched = state.dispatcher.dispatch(rule.node_id, restart_command(id));
    if !dispatched {
        tracing::warn!(node_id = rule.node_id, rule_id = id, "agent offline; restart skipped");
    }
    Ok(Json(json!({ "ok": true, "dispatched": dispatched })))
}

pub async fn stats(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<RuleStatsResponse>> {
    let rule = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    ensure_can_touch(&auth, &rule)?;

    let series = sqlx::query_as::<_, RuleStatsBucket>(
        "SELECT bucket_at, rx_bytes, tx_bytes, connection_count, error_count \
         FROM rule_stats WHERE rule_id = ? ORDER BY bucket_at DESC LIMIT 144",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(RuleStatsResponse {
        current: RuleStatsCurrent {
            enabled: rule.enabled != 0,
            rx_bytes: rule.rx_bytes,
            tx_bytes: rule.tx_bytes,
            connection_count: rule.connection_count,
        },
        series,
    }))
}

pub async fn logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<RuleLogEntry>>> {
    // 软删的规则也能查 audit(历史是 audit 的核心场景)。
    // owner 校验仍要做:查 forward_rules(忽略 deleted_at)。
    // SELECT 不再走 find_by_id(它过滤 deleted_at IS NULL),直接读 user_id。
    let owner: Option<(i64,)> =
        sqlx::query_as("SELECT user_id FROM forward_rules WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (rule_user_id,) = owner.ok_or(ApiError::NotFound)?;
    if !auth.is_admin() && rule_user_id != auth.0.sub {
        return Err(ApiError::NotFound);
    }

    let entries = sqlx::query_as::<_, RuleLogEntry>(
        "SELECT id, actor_user_id, action, result, error_message, created_at \
         FROM audit_logs WHERE target_type = 'rule' AND target_id = ? \
         ORDER BY id DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(entries))
}

fn validate_protocol(p: &str) -> ApiResult<()> {
    if matches!(p, "tcp" | "udp" | "tcp_udp") {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "protocol must be tcp | udp | tcp_udp".into(),
        ))
    }
}

/// 新建/改动规则时与同 (node_id, listen_ip, listen_port) 上的活跃规则做协议互斥校验。
/// UNIQUE 索引按 protocol 字符串严格匹配,所以 tcp_udp 和 tcp/udp 在 DB 层不互斥,但实际 Agent
/// bind 会冲突。规则: tcp ↔ tcp_udp 冲突, udp ↔ tcp_udp 冲突, tcp ↔ udp 不冲突。
/// `exclude_id` 用于 update,排除自身。
async fn ensure_no_protocol_conflict(
    pool: &sqlx::SqlitePool,
    node_id: i64,
    listen_ip: &str,
    listen_port: i64,
    new_protocol: &str,
    exclude_id: Option<i64>,
) -> ApiResult<()> {
    let conflicts: &[&str] = match new_protocol {
        "tcp" => &["tcp_udp"],
        "udp" => &["tcp_udp"],
        "tcp_udp" => &["tcp", "udp"],
        _ => return Ok(()),
    };
    let placeholders = vec!["?"; conflicts.len()].join(",");
    let sql = format!(
        "SELECT id FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND listen_port = ? \
           AND protocol IN ({}) AND deleted_at IS NULL{} LIMIT 1",
        placeholders,
        if exclude_id.is_some() { " AND id != ?" } else { "" }
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql)
        .bind(node_id)
        .bind(listen_ip)
        .bind(listen_port);
    for p in conflicts {
        q = q.bind(*p);
    }
    if let Some(eid) = exclude_id {
        q = q.bind(eid);
    }
    if q.fetch_optional(pool).await?.is_some() {
        return Err(ApiError::BadRequest(format!(
            "listen_port {listen_port} on this node conflicts with an existing rule \
             (tcp_udp mutually excludes tcp/udp on the same port)"
        )));
    }
    Ok(())
}

/// 自动分配:node 池内最小可用 listen_port。
/// 占用集合 = 同 node + 同 listen_ip 的活跃规则,按协议互斥语义判定:
/// tcp ↔ {tcp, tcp_udp} / udp ↔ {udp, tcp_udp} / tcp_udp ↔ 全部。
/// 并发窗口与 ensure_no_protocol_conflict 相同:精确重复由 DB UNIQUE 兜底,
/// 互斥型并发与既有 create 行为一致(MVP 已接受)。
async fn allocate_port(
    pool: &sqlx::SqlitePool,
    node: &Node,
    listen_ip: &str,
    protocol: &str,
    reserved: &[i64],
) -> ApiResult<i64> {
    let taken: Vec<(i64, String)> = sqlx::query_as(
        "SELECT listen_port, protocol FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND deleted_at IS NULL \
           AND listen_port BETWEEN ? AND ?",
    )
    .bind(node.id)
    .bind(listen_ip)
    .bind(node.port_pool_min)
    .bind(node.port_pool_max)
    .fetch_all(pool)
    .await?;

    let conflicts = |existing: &str| -> bool {
        match protocol {
            "tcp" => matches!(existing, "tcp" | "tcp_udp"),
            "udp" => matches!(existing, "udp" | "tcp_udp"),
            _ => true, // tcp_udp 与所有协议互斥
        }
    };
    let blocked: std::collections::HashSet<i64> = taken
        .iter()
        .filter(|(_, proto)| conflicts(proto))
        .map(|(port, _)| *port)
        .collect();

    for port in node.port_pool_min..=node.port_pool_max {
        if !reserved.contains(&port) && !blocked.contains(&port) {
            return Ok(port);
        }
    }
    Err(ApiError::BadRequest(format!(
        "port pool exhausted on node {} [{}-{}]",
        node.id, node.port_pool_min, node.port_pool_max
    )))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest(
                "rule binding already exists (same node/protocol/listen_ip/listen_port)".into(),
            );
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("rule fields violate check constraint".into());
        }
        if db_err.is_foreign_key_violation() {
            return ApiError::BadRequest("node_id or user_id does not exist".into());
        }
    }
    ApiError::Database(e)
}
