use crate::{
    audit,
    auth::{
        extractor::{ActorIp, AuthUser},
        jwt::decode_jwt,
        token::{generate_token, hash_token},
    },
    error::{ApiError, ApiResult},
    models::{
        grant,
        node::{Node, SORT_FIELDS},
    },
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::prelude::FromRow;
use std::convert::Infallible;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

#[derive(Serialize)]
pub struct NodeView {
    pub id: i64,
    pub name: String,
    pub region: String,
    /// 接入地址(互联实际使用);用户视角被替换为有效展示地址。
    pub public_ip: String,
    /// 展示地址(可选,空=回落接入地址);用户视角恒为空串。
    pub display_address: String,
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
    pub agent_version: String,
    /// 协议嗅探阻断位掩码:bit0=http(1) bit1=tls(2) bit2=socks(4);0=不阻断。
    pub block_protocols: i64,
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
            display_address: n.display_address,
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
            agent_version: n.agent_version,
            block_protocols: n.block_protocols,
            created_at: n.created_at,
            updated_at: n.updated_at,
        }
    }
}

impl NodeView {
    /// 普通用户视角:保留自助建规则所需(身份/在线状态/端口池/入口地址),
    /// 抹掉运维指标与控制面信息。JSON 形状不变,前端类型零分叉。
    /// P8: public_ip 替换为有效展示地址(展示地址非空用之,否则回落接入地址),
    /// 接入地址本体不对用户暴露。
    fn sanitize_for_user(mut self) -> Self {
        if !self.display_address.trim().is_empty() {
            self.public_ip = self.display_address.trim().to_string();
        }
        self.display_address = String::new();
        self.grpc_endpoint = String::new();
        self.agent_version = String::new();
        self.block_protocols = 0;
        self.cpu_usage = 0.0;
        self.memory_usage = 0.0;
        self.load_average = 0.0;
        self.rx_bytes_total = 0;
        self.tx_bytes_total = 0;
        self
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub search: Option<String>,
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
    pub display_address: String,
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
    /// mTLS 四件套(一次性):CA 公钥 / 该节点 client 证书 / client 私钥。
    pub ca_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

#[derive(Deserialize)]
pub struct UpdateNodeRequest {
    pub name: Option<String>,
    pub region: Option<String>,
    pub public_ip: Option<String>,
    pub display_address: Option<String>,
    pub grpc_endpoint: Option<String>,
    pub port_pool_min: Option<u16>,
    pub port_pool_max: Option<u16>,
    /// 协议嗅探阻断位掩码 0-7(bit0=http bit1=tls bit2=socks);None 不改。
    pub block_protocols: Option<i64>,
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
    // 放行普通用户(自助建规则需要节点列表),但响应经 sanitize_for_user 净化。
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);

    // 非 admin:只返回被授权的节点(默认拒绝),供自助建规则选节点;响应净化敏感字段。
    // 授权节点数有限,逐个取即可,不分页(前端建规则下拉需要全部授权节点)。
    if !auth.is_admin() {
        let ids = grant::granted_node_ids(&state.pool, auth.0.sub).await?;
        let mut items = Vec::new();
        for nid in &ids {
            if let Some(n) = Node::find_by_id(&state.pool, *nid).await? {
                items.push(NodeView::from(n).sanitize_for_user());
            }
        }
        let total = items.len() as i64;
        return Ok(Json(NodeListResponse { items, total, page, page_size }));
    }

    let sort_field = q.sort.as_deref().unwrap_or("id");
    if !SORT_FIELDS.contains(&sort_field) {
        return Err(ApiError::BadRequest(format!(
            "排序字段不合法,可用: {}",
            SORT_FIELDS.join(",")
        )));
    }
    let order_desc = match q.order.as_deref().unwrap_or("desc") {
        "asc" => false,
        "desc" => true,
        _ => return Err(ApiError::BadRequest("排序方向必须是 asc 或 desc".into())),
    };
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let nodes =
        Node::list_paged(&state.pool, sort_field, order_desc, page_size, offset, search).await?;
    let total = Node::count(&state.pool, search).await?;

    let sanitize = !auth.is_admin();
    Ok(Json(NodeListResponse {
        items: nodes
            .into_iter()
            .map(NodeView::from)
            .map(|v| if sanitize { v.sanitize_for_user() } else { v })
            .collect(),
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
    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // 非 admin 只能看被授权的节点(未授权按不存在处理,不泄露)。
    if !auth.is_admin() && !grant::node_granted(&state.pool, auth.0.sub, id).await? {
        return Err(ApiError::NotFound);
    }
    let view = NodeView::from(node);
    Ok(Json(if auth.is_admin() {
        view
    } else {
        view.sanitize_for_user()
    }))
}

/// P10b: 向在线节点下发 Agent 升级命令。Agent 自行下载/校验/原子替换/exec 重启。
/// 二进制来源 = ${PANEL_DATA_DIR}/agent-dist/node-agent-linux-{amd64,arm64},
/// sha256 现算(文件可能被发版替换,不缓存)。
pub async fn upgrade_agent(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let base_url = state
        .config
        .panel_public_base_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            ApiError::BadRequest("未配置 PANEL_PUBLIC_BASE_URL,Agent 无法定位下载地址".into())
        })?;

    let dist = std::path::Path::new(&state.config.panel_data_dir).join("agent-dist");
    let sha_of = |name: &str| -> Option<String> {
        use sha2::{Digest, Sha256};
        std::fs::read(dist.join(name))
            .ok()
            .map(|b| hex::encode(Sha256::digest(&b)))
    };
    let sha256_amd64 = sha_of("node-agent-linux-amd64").unwrap_or_default();
    let sha256_arm64 = sha_of("node-agent-linux-arm64").unwrap_or_default();
    if sha256_amd64.is_empty() && sha256_arm64.is_empty() {
        return Err(ApiError::BadRequest(
            "面板 agent-dist 目录无任何 Agent 二进制,无法升级".into(),
        ));
    }

    let version = env!("CARGO_PKG_VERSION").to_string();
    let cmd = emorelay_common::control::v1::Command {
        body: Some(emorelay_common::control::v1::command::Body::UpgradeAgent(
            emorelay_common::control::v1::UpgradeAgent {
                version: version.clone(),
                base_url,
                sha256_amd64,
                sha256_arm64,
            },
        )),
    };
    if !state.dispatcher.dispatch(id, cmd) {
        // 失败尝试也落 audit(危险操作的未遂记录)。
        audit::record_with_ip(
            &state.pool,
            Some(auth.0.sub),
            actor_ip.as_option(),
            "node.upgrade_agent",
            Some("node"),
            Some(id),
            Some("dispatch failed: offline"),
            false,
            Some("节点不在线"),
        )
        .await;
        return Err(ApiError::BadRequest(
            "节点不在线,无法下发升级命令".into(),
        ));
    }

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "node.upgrade_agent",
        Some("node"),
        Some(id),
        Some(&format!("{} -> {version}", node.agent_version)),
        true,
        None,
    )
    .await;

    Ok(Json(json!({ "ok": true, "dispatched": true, "target_version": version })))
}

/// 某节点被授权给哪些用户(节点详情页反向显示)。admin only。
pub async fn grants(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<crate::models::grant::GrantedUser>>> {
    auth.require_admin()?;
    Ok(Json(grant::users_for_node(&state.pool, id).await?))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateNodeRequest>,
) -> ApiResult<Json<CreateNodeResponse>> {
    auth.require_admin()?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("名称不能为空".into()));
    }
    let (port_min, port_max) =
        normalize_port_pool(req.port_pool_min, req.port_pool_max, 10000, 65535)?;

    let token = generate_token();
    let token_hash = hash_token(&token);

    let id = Node::create(
        &state.pool,
        req.name.trim(),
        &req.region,
        &req.public_ip,
        req.display_address.trim(),
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

    // 为新节点签发 mTLS client 证书(四件套之三);DB 只存 serial/fingerprint。
    let issued = crate::tls::issue::issue_client_cert(&state.ca, id)
        .map_err(ApiError::Internal)?;
    Node::set_cert_meta(&state.pool, id, &issued.serial, &issued.fingerprint).await?;

    Ok(Json(CreateNodeResponse {
        node: node.into(),
        agent_token: token,
        ca_pem: state.ca.ca_pem.clone(),
        client_cert_pem: issued.cert_pem,
        client_key_pem: issued.key_pem,
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
        return Err(ApiError::BadRequest("名称不能为空".into()));
    }
    let port_min = req.port_pool_min.map(i64::from);
    let port_max = req.port_pool_max.map(i64::from);
    if let (Some(lo), Some(hi)) = (port_min, port_max) {
        if lo < 1 || hi < 1 || lo > hi {
            return Err(ApiError::BadRequest(
                "端口池上下界必须在 1-65535 之间且下界不大于上界".into(),
            ));
        }
    } else if port_min.is_some() || port_max.is_some() {
        // 仅给一端时也要确保它合法（>=1 由 u16 保证 0-65535，需排除 0）
        if matches!(port_min, Some(v) if v == 0) || matches!(port_max, Some(v) if v == 0) {
            return Err(ApiError::BadRequest(
                "端口池边界必须在 1-65535 之间".into(),
            ));
        }
    }

    if let Some(m) = req.block_protocols {
        if !(0..=7).contains(&m) {
            return Err(ApiError::BadRequest(
                "协议阻断掩码必须在 0-7 之间(bit0=http bit1=tls bit2=socks)".into(),
            ));
        }
    }

    let rows = Node::update(
        &state.pool,
        id,
        trimmed_name,
        req.region.as_deref(),
        req.public_ip.as_deref(),
        // 传 "" 即清空展示地址(回落接入地址),None 不动。
        req.display_address.as_deref().map(str::trim),
        req.grpc_endpoint.as_deref(),
        port_min,
        port_max,
    )
    .await
    .map_err(map_sqlx_to_api)?;

    if rows == 0 {
        return Err(ApiError::NotFound);
    }

    // 协议阻断掩码变更:落库后重发该节点全部非隧道规则,让 Agent 拿到新掩码。
    if let Some(m) = req.block_protocols {
        Node::set_block_protocols(&state.pool, id, m).await?;
        for rule in crate::models::rule::Rule::list_active_for_node(&state.pool, id).await? {
            if rule.tunnel_id.is_none() {
                let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;
            }
        }
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

#[derive(Deserialize)]
pub struct StreamQuery {
    pub token: Option<String>,
}

/// 节点实时事件 SSE(admin only)。订阅 node_events 广播,每个变更 node_id 拉取该
/// 节点最新快照推给客户端,替代前端轮询(前端仍保留轮询兜底)。EventSource 取不到
/// header,故鉴权走 ?token=<jwt>(回落 Authorization: Bearer);校验角色为 admin。
pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<StreamQuery>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string)
        .or(q.token)
        .ok_or(ApiError::Unauthorized)?;
    let claims = decode_jwt(&state.config.jwt_secret, &token).map_err(|_| ApiError::Unauthorized)?;
    if claims.role != "admin" {
        return Err(ApiError::Forbidden);
    }

    let pool = state.pool.clone();
    let rx = state.node_events.subscribe();
    // BroadcastStream 滞后(慢消费者)时丢 lagged 错误继续;每事件拉节点快照转 SSE。
    let stream = BroadcastStream::new(rx)
        .then(move |res| {
            let pool = pool.clone();
            async move {
                let node_id = res.ok()?;
                let node = Node::find_by_id(&pool, node_id).await.ok().flatten()?;
                let json = serde_json::to_string(&NodeView::from(node)).ok()?;
                Some(Event::default().event("node").data(json))
            }
        })
        .filter_map(|opt: Option<Event>| opt.map(Ok::<_, Infallible>));

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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

    // 节点参与任一活跃隧道 → 拒删(P3b)。
    if crate::models::tunnel::TunnelHop::node_in_active_tunnel(&state.pool, id).await? {
        return Err(ApiError::BadRequest(
            "节点正参与活跃隧道,请先删除相关隧道".into()));
    }

    let rows = Node::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    // 防删除后旧 session 残留仍可上报:立即失效该 node 的全部在线 session(H3)。
    state.sessions.revoke_node(id);
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

pub async fn revoke_credentials(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _node = Node::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // 取旧 fingerprint 入 CRL(若有)。
    let old: Option<(Option<String>,)> =
        sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ? AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    if let Some((Some(old_fp),)) = old {
        let crl_path = format!("{}/tls/crl.json", state.config.panel_data_dir);
        state.crl.revoke(&old_fp, &crl_path).map_err(ApiError::Internal)?;
    }

    // 重签新证书 + 落新 meta。
    let issued = crate::tls::issue::issue_client_cert(&state.ca, id).map_err(ApiError::Internal)?;
    Node::set_cert_meta(&state.pool, id, &issued.serial, &issued.fingerprint).await?;

    // 吊销凭据后立即踢掉该 node 的在线 session,使吊销即时生效(H3)。
    state.sessions.revoke_node(id);

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "node.credentials_revoked",
        Some("node"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    Ok(Json(json!({
        "ca_pem": state.ca.ca_pem.clone(),
        "client_cert_pem": issued.cert_pem,
        "client_key_pem": issued.key_pem,
    })))
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
            "端口池边界必须在 1-65535 之间".into(),
        ));
    }
    if lo > hi {
        return Err(ApiError::BadRequest(
            "端口池下界不能大于上界".into(),
        ));
    }
    Ok((i64::from(lo), i64::from(hi)))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest("节点名称已存在".into());
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("节点字段不满足约束".into());
        }
    }
    ApiError::Database(e)
}
