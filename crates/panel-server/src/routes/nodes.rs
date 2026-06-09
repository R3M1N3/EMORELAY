use crate::{
    audit,
    auth::{
        extractor::{ActorIp, AuthUser},
        token::{generate_token, hash_token},
    },
    error::{ApiError, ApiResult},
    models::node::{Node, SORT_FIELDS},
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::prelude::FromRow;

#[derive(Serialize)]
pub struct NodeView {
    pub id: i64,
    pub name: String,
    pub region: String,
    pub public_ip: String,
    pub grpc_endpoint: String,
    pub status: String,
    pub last_seen_at: Option<String>,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
    pub rx_bytes_total: i64,
    pub tx_bytes_total: i64,
    pub port_pool_min: i64,
    pub port_pool_max: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Node> for NodeView {
    fn from(n: Node) -> Self {
        Self {
            id: n.id,
            name: n.name,
            region: n.region,
            public_ip: n.public_ip,
            grpc_endpoint: n.grpc_endpoint,
            status: n.status,
            last_seen_at: n.last_seen_at,
            cpu_usage: n.cpu_usage,
            memory_usage: n.memory_usage,
            load_average: n.load_average,
            rx_bytes_total: n.rx_bytes_total,
            tx_bytes_total: n.tx_bytes_total,
            port_pool_min: n.port_pool_min,
            port_pool_max: n.port_pool_max,
            created_at: n.created_at,
            updated_at: n.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Serialize)]
pub struct NodeListResponse {
    pub items: Vec<NodeView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateNodeRequest {
    pub name: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub public_ip: String,
    #[serde(default)]
    pub grpc_endpoint: String,
    pub port_pool_min: Option<u16>,
    pub port_pool_max: Option<u16>,
}

#[derive(Serialize)]
pub struct CreateNodeResponse {
    pub node: NodeView,
    /// 仅在创建时返回一次的明文 token；之后只能轮换重新发放。
    pub agent_token: String,
}

#[derive(Deserialize)]
pub struct UpdateNodeRequest {
    pub name: Option<String>,
    pub region: Option<String>,
    pub public_ip: Option<String>,
    pub grpc_endpoint: Option<String>,
    pub port_pool_min: Option<u16>,
    pub port_pool_max: Option<u16>,
}

#[derive(Serialize, FromRow)]
pub struct NodeStatsBucket {
    pub bucket_at: String,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
}

#[derive(Serialize)]
pub struct NodeStatsCurrent {
    pub status: String,
    pub last_seen_at: Option<String>,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
    pub rx_bytes_total: i64,
    pub tx_bytes_total: i64,
}

#[derive(Serialize)]
pub struct NodeStatsResponse {
    pub current: NodeStatsCurrent,
    /// node_stats 由单元 L Agent 心跳上报后填充；本阶段返回空列表是预期行为。
    pub series: Vec<NodeStatsBucket>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<NodeListResponse>> {
    auth.require_admin()?;
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

    let nodes = Node::list_paged(&state.pool, sort_field, order_desc, page_size, offset).await?;
    let total = Node::count(&state.pool).await?;

    Ok(Json(NodeListResponse {
        items: nodes.into_iter().map(Into::into).collect(),
        total,
        page,
        page_size,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<NodeView>> {
    auth.require_admin()?;
    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(node.into()))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateNodeRequest>,
) -> ApiResult<Json<CreateNodeResponse>> {
    auth.require_admin()?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let (port_min, port_max) =
        normalize_port_pool(req.port_pool_min, req.port_pool_max, 1, 65535)?;

    let token = generate_token();
    let token_hash = hash_token(&token);

    let id = Node::create(
        &state.pool,
        req.name.trim(),
        &req.region,
        &req.public_ip,
        &req.grpc_endpoint,
        &token_hash,
        port_min,
        port_max,
    )
    .await
    .map_err(map_sqlx_to_api)?;

    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "node.create",
        Some("node"),
        Some(id),
        Some(req.name.trim()),
        true,
        None,
    )
    .await;

    Ok(Json(CreateNodeResponse {
        node: node.into(),
        agent_token: token,
    }))
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    Json(req): Json<UpdateNodeRequest>,
) -> ApiResult<Json<NodeView>> {
    auth.require_admin()?;
    let trimmed_name = req.name.as_deref().map(str::trim);
    if matches!(trimmed_name, Some(n) if n.is_empty()) {
        return Err(ApiError::BadRequest("name cannot be empty".into()));
    }
    let port_min = req.port_pool_min.map(i64::from);
    let port_max = req.port_pool_max.map(i64::from);
    if let (Some(lo), Some(hi)) = (port_min, port_max) {
        if lo < 1 || hi < 1 || lo > hi {
            return Err(ApiError::BadRequest(
                "port_pool_min/max must be 1-65535 and min<=max".into(),
            ));
        }
    } else if port_min.is_some() || port_max.is_some() {
        // 仅给一端时也要确保它合法（>=1 由 u16 保证 0-65535，需排除 0）
        if matches!(port_min, Some(v) if v == 0) || matches!(port_max, Some(v) if v == 0) {
            return Err(ApiError::BadRequest(
                "port_pool bounds must be 1-65535".into(),
            ));
        }
    }

    let rows = Node::update(
        &state.pool,
        id,
        trimmed_name,
        req.region.as_deref(),
        req.public_ip.as_deref(),
        req.grpc_endpoint.as_deref(),
        port_min,
        port_max,
    )
    .await
    .map_err(map_sqlx_to_api)?;

    if rows == 0 {
        return Err(ApiError::NotFound);
    }

    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "node.update",
        Some("node"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    Ok(Json(node.into()))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;

    // 防呆：节点上仍有活跃规则时拒绝删除(plan §2.3 P1)。
    #[derive(FromRow)]
    struct ConflictRule {
        id: i64,
        name: String,
    }

    let conflicts: Vec<ConflictRule> = sqlx::query_as(
        "SELECT id, name FROM forward_rules \
         WHERE node_id = ? AND deleted_at IS NULL \
         ORDER BY id LIMIT 4",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    if !conflicts.is_empty() {
        let shown = conflicts
            .iter()
            .take(3)
            .map(|r| format!("#{}({})", r.id, r.name))
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if conflicts.len() > 3 {
            format!("...还有 {} 条", conflicts.len() - 3)
        } else {
            String::new()
        };
        return Err(ApiError::BadRequest(format!(
            "节点上仍有活跃规则,请先删除: {shown}{suffix}"
        )));
    }

    let rows = Node::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "node.delete",
        Some("node"),
        Some(id),
        None,
        true,
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn stats(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<NodeStatsResponse>> {
    auth.require_admin()?;
    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let series = sqlx::query_as::<_, NodeStatsBucket>(
        "SELECT bucket_at, cpu_usage, memory_usage, load_average, rx_bytes, tx_bytes \
         FROM node_stats WHERE node_id = ? ORDER BY bucket_at DESC LIMIT 144",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(NodeStatsResponse {
        current: NodeStatsCurrent {
            status: node.status,
            last_seen_at: node.last_seen_at,
            cpu_usage: node.cpu_usage,
            memory_usage: node.memory_usage,
            load_average: node.load_average,
            rx_bytes_total: node.rx_bytes_total,
            tx_bytes_total: node.tx_bytes_total,
        },
        series,
    }))
}

fn normalize_port_pool(
    min: Option<u16>,
    max: Option<u16>,
    default_min: u16,
    default_max: u16,
) -> ApiResult<(i64, i64)> {
    let lo = min.unwrap_or(default_min);
    let hi = max.unwrap_or(default_max);
    if lo == 0 || hi == 0 {
        return Err(ApiError::BadRequest(
            "port_pool bounds must be 1-65535".into(),
        ));
    }
    if lo > hi {
        return Err(ApiError::BadRequest(
            "port_pool_min must be <= port_pool_max".into(),
        ));
    }
    Ok((i64::from(lo), i64::from(hi)))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest("node name already exists".into());
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("invalid node fields (check constraint)".into());
        }
    }
    ApiError::Database(e)
}
