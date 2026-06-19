use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::{grant, settings, tunnel::{Tunnel, TunnelHop}},
    state::AppState,
};
use axum::{extract::{Path, Query, State}, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize)]
pub struct TunnelView {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub traffic_ratio: f64,
    pub billing_mode: i64,
    pub hops_count: i64,
    pub rules_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct HopView {
    pub ordinal: i64,
    pub node_id: i64,
    pub inter_port: Option<i64>,
}

/// 隧道详情里的关联规则摘要(前端详情页列表用,不暴露完整 RuleView)。
#[derive(Serialize)]
pub struct TunnelRuleRef {
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub listen_port: i64,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct TunnelDetail {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub status: String,
    pub traffic_ratio: f64,
    pub billing_mode: i64,
    pub hops: Vec<HopView>,
    pub rules_count: i64,
    pub rules: Vec<TunnelRuleRef>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct ListQuery { pub page: Option<i64>, pub page_size: Option<i64> }

#[derive(Serialize)]
pub struct TunnelListResponse {
    pub items: Vec<TunnelView>, pub total: i64, pub page: i64, pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateTunnelRequest {
    pub name: String,
    pub transport: String,
    pub node_ids: Vec<i64>,
    /// 计费倍率(默认 1.0);billing_mode 1=单向 2=双向(默认 2)。
    pub traffic_ratio: Option<f64>,
    pub billing_mode: Option<i64>,
}

#[derive(Deserialize)]
pub struct UpdateTunnelRequest {
    pub name: Option<String>,
    pub traffic_ratio: Option<f64>,
    pub billing_mode: Option<i64>,
}

/// 校验计费参数:倍率 0..=100,模式 1|2。返回规范化后的值(None 表示不改)。
fn validate_billing(
    ratio: Option<f64>,
    mode: Option<i64>,
) -> ApiResult<()> {
    if let Some(r) = ratio {
        if !(r.is_finite() && (0.0..=100.0).contains(&r)) {
            return Err(ApiError::BadRequest("流量倍率必须在 0 到 100 之间".into()));
        }
    }
    if let Some(m) = mode {
        if m != 1 && m != 2 {
            return Err(ApiError::BadRequest("计费模式必须是 1(单向)或 2(双向)".into()));
        }
    }
    Ok(())
}

pub async fn list(
    State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>,
) -> ApiResult<Json<TunnelListResponse>> {
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    // 非 admin:只返回被授权的隧道(默认拒绝);admin 走分页全量。
    let (tunnels, total) = if auth.is_admin() {
        let t = Tunnel::list_paged(&state.pool, page_size, offset).await?;
        let c = Tunnel::count(&state.pool).await?;
        (t, c)
    } else {
        // 一次 IN 查询取回全部授权隧道,替代逐条 find_by_id。
        let ids = grant::granted_tunnel_ids(&state.pool, auth.0.sub).await?;
        let t = Tunnel::list_by_ids(&state.pool, &ids).await?;
        let c = t.len() as i64;
        (t, c)
    };
    // 批量取本页隧道的 hop 数 + 规则数(两次 IN+GROUP BY),替代逐隧道 2N 次子查询。
    let tids: Vec<i64> = tunnels.iter().map(|t| t.id).collect();
    let hops_count = Tunnel::hops_count_by_ids(&state.pool, &tids).await?;
    let rules_count = Tunnel::rules_count_by_ids(&state.pool, &tids).await?;
    let items = tunnels
        .into_iter()
        .map(|t| {
            let hops = hops_count.get(&t.id).copied().unwrap_or(0);
            let rules = rules_count.get(&t.id).copied().unwrap_or(0);
            TunnelView {
                id: t.id, name: t.name, transport: t.transport, status: t.status,
                traffic_ratio: t.traffic_ratio, billing_mode: t.billing_mode,
                hops_count: hops, rules_count: rules,
                created_at: t.created_at, updated_at: t.updated_at,
            }
        })
        .collect();
    Ok(Json(TunnelListResponse { items, total, page, page_size }))
}

pub async fn get(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<TunnelDetail>> {
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    // 非 admin 只能看被授权的隧道(未授权按不存在处理)。
    if !auth.is_admin() && !grant::tunnel_granted(&state.pool, auth.0.sub, id).await? {
        return Err(ApiError::NotFound);
    }
    let status = Tunnel::compute_status(&state.pool, id).await?;
    let _ = Tunnel::set_status(&state.pool, id, &status).await;
    let hops = TunnelHop::list_for_tunnel(&state.pool, id).await?;
    let rules: Vec<TunnelRuleRef> = crate::models::rule::Rule::list_active_for_tunnel(&state.pool, id)
        .await?
        .into_iter()
        .map(|r| TunnelRuleRef {
            id: r.id,
            name: r.name,
            protocol: r.protocol,
            listen_port: r.listen_port,
            enabled: r.enabled != 0,
        })
        .collect();
    Ok(Json(TunnelDetail {
        id: t.id, name: t.name, transport: t.transport, status,
        traffic_ratio: t.traffic_ratio, billing_mode: t.billing_mode,
        hops: hops.into_iter().map(|h| HopView { ordinal: h.ordinal, node_id: h.node_id, inter_port: h.inter_port }).collect(),
        rules_count: rules.len() as i64,
        rules,
        created_at: t.created_at, updated_at: t.updated_at,
    }))
}

/// 某隧道被授权给哪些用户(隧道详情页反向显示)。admin only。
pub async fn grants(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<Vec<crate::models::grant::GrantedUser>>> {
    auth.require_admin()?;
    Ok(Json(grant::users_for_tunnel(&state.pool, id).await?))
}

pub async fn create(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp,
    Json(req): Json<CreateTunnelRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("名称不能为空".into()));
    }
    if !matches!(req.transport.as_str(), "tcp" | "tls" | "wss") {
        return Err(ApiError::BadRequest("传输协议必须是 tcp | tls | wss".into()));
    }
    validate_billing(req.traffic_ratio, req.billing_mode)?;
    if req.node_ids.len() < 2 {
        return Err(ApiError::BadRequest("隧道至少需要 2 个节点".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for nid in &req.node_ids {
        if !seen.insert(*nid) {
            return Err(ApiError::BadRequest("节点链中不能出现重复节点".into()));
        }
    }
    #[derive(sqlx::FromRow)]
    struct NodeRow { id: i64, status: String, public_ip: String, port_pool_min: i64, port_pool_max: i64 }
    let reserved = settings::reserved_ports(&state.pool).await;
    let mut pools: std::collections::HashMap<i64, (i64, i64)> = std::collections::HashMap::new();
    for (ordinal, nid) in req.node_ids.iter().enumerate() {
        let row: Option<NodeRow> = sqlx::query_as(
            "SELECT id, status, public_ip, port_pool_min, port_pool_max FROM nodes WHERE id = ? AND deleted_at IS NULL",
        ).bind(nid).fetch_optional(&state.pool).await?;
        let row = row.ok_or_else(|| ApiError::BadRequest(format!("节点 {nid} 不存在")))?;
        if row.status != "online" {
            return Err(ApiError::BadRequest(
                "请确保链上所有节点都在线".into(),
            ));
        }
        // ordinal ≥ 1 的 hop 会被上一跳 dial,split 时 next_hop_addr 取它的 public_ip。
        if ordinal >= 1 && row.public_ip.trim().is_empty() {
            return Err(ApiError::BadRequest(format!(
                "节点 {nid} 作为第 2 跳起的中继必须配置公网 IP"
            )));
        }
        pools.insert(row.id, (row.port_pool_min, row.port_pool_max));
    }
    let mut hops: Vec<(i64, i64, Option<i64>)> = Vec::with_capacity(req.node_ids.len());
    for (ordinal, nid) in req.node_ids.iter().enumerate() {
        if ordinal == 0 {
            hops.push((0, *nid, None));
            continue;
        }
        let (lo, hi) = pools[nid];
        let taken: Vec<i64> = sqlx::query_scalar(
            "SELECT listen_port FROM forward_rules WHERE node_id = ? AND deleted_at IS NULL \
             UNION SELECT th.inter_port FROM tunnel_hops th JOIN tunnels t ON t.id = th.tunnel_id \
             WHERE th.node_id = ? AND th.inter_port IS NOT NULL AND t.deleted_at IS NULL",
        ).bind(nid).bind(nid).fetch_all(&state.pool).await?;
        let already: std::collections::HashSet<i64> =
            hops.iter().filter(|(_, n, _)| n == nid).filter_map(|(_, _, p)| *p).collect();
        let port = (lo..=hi).find(|p| {
            !reserved.contains(p) && !taken.contains(p) && !already.contains(p)
        }).ok_or_else(|| ApiError::BadRequest(format!(
            "节点 {nid} 端口池已无可用中继端口"
        )))?;
        hops.push((ordinal as i64, *nid, Some(port)));
    }

    let tid = Tunnel::create_with_hops(&state.pool, name, &req.transport, &hops)
        .await
        .map_err(map_sqlx_to_api)?;
    // 非默认计费参数随后覆盖(create_with_hops 取 DB 默认 1.0/双向)。
    if req.traffic_ratio.is_some() || req.billing_mode.is_some() {
        Tunnel::update_fields(&state.pool, tid, None, req.traffic_ratio, req.billing_mode)
            .await
            .map_err(map_sqlx_to_api)?;
    }

    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.create", Some("tunnel"), Some(tid), Some(name), true, None).await;

    // 下发失败只 warn(实体已落库,reconcile 兜底),不让误导性 500 回给客户端。
    if let Some(t) = Tunnel::find_by_id(&state.pool, tid).await? {
        let _ = crate::grpc::tunnel_dispatch::dispatch_tunnel_credentials(&state, &t).await;
    }

    Ok(Json(json!({ "id": tid })))
}

pub async fn update(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp,
    Path(id): Path<i64>, Json(req): Json<UpdateTunnelRequest>,
) -> ApiResult<Json<TunnelView>> {
    auth.require_admin()?;
    if let Some(name) = req.name.as_deref() {
        if name.trim().is_empty() {
            return Err(ApiError::BadRequest("名称不能为空".into()));
        }
    }
    validate_billing(req.traffic_ratio, req.billing_mode)?;
    let name = req.name.as_deref().map(str::trim);
    if name.is_some() || req.traffic_ratio.is_some() || req.billing_mode.is_some() {
        let rows = Tunnel::update_fields(&state.pool, id, name, req.traffic_ratio, req.billing_mode)
            .await
            .map_err(map_sqlx_to_api)?;
        if rows == 0 { return Err(ApiError::NotFound); }
    }
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let hops = TunnelHop::list_for_tunnel(&state.pool, id).await?;
    let rules_count = Tunnel::active_rule_refs(&state.pool, id).await?;
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.update", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(TunnelView {
        id: t.id, name: t.name, transport: t.transport, status: t.status,
        traffic_ratio: t.traffic_ratio, billing_mode: t.billing_mode,
        hops_count: hops.len() as i64, rules_count, created_at: t.created_at, updated_at: t.updated_at,
    }))
}

pub async fn delete(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let refs = Tunnel::active_rule_refs(&state.pool, id).await?;
    if refs > 0 {
        return Err(ApiError::BadRequest(format!(
            "隧道仍被 {refs} 条规则关联,请先解除关联"
        )));
    }
    let hop_nodes: Vec<i64> =
        sqlx::query_scalar("SELECT node_id FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
    let rows = Tunnel::soft_delete(&state.pool, id).await?;
    if rows == 0 { return Err(ApiError::NotFound); }
    crate::grpc::tunnel_dispatch::dispatch_revoke_tunnel_credentials(&state, id, &hop_nodes);
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.delete", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn restart(
    State(state): State<AppState>, auth: AuthUser, actor_ip: ActorIp, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    // 凭据先行(重签轮换),再对该隧道全部活跃规则 per-hop restart(与轮换 sweeper 共用)。
    let dispatched =
        crate::grpc::tunnel_dispatch::rotate_credentials_and_restart(&state, &t).await?;
    audit::record_with_ip(&state.pool, Some(auth.0.sub), actor_ip.as_option(),
        "tunnel.restart", Some("tunnel"), Some(id), None, true, None).await;
    Ok(Json(json!({ "ok": true, "dispatched": dispatched })))
}

pub async fn status(
    State(state): State<AppState>, auth: AuthUser, Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let _t = Tunnel::find_by_id(&state.pool, id).await?.ok_or(ApiError::NotFound)?;
    let status = Tunnel::compute_status(&state.pool, id).await?;
    let _ = Tunnel::set_status(&state.pool, id, &status).await;
    Ok(Json(json!({ "id": id, "status": status })))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db) = e.as_database_error() {
        if db.is_unique_violation() {
            // SQLite unique violation 消息含冲突索引/列名;区分 inter_port 撞(并发)与 name 撞。
            if db.message().contains("inter_port") {
                return ApiError::BadRequest(
                    "中继端口分配冲突(可能有并发创建),请重试".into());
            }
            return ApiError::BadRequest("隧道名称已存在".into());
        }
        if db.is_check_violation() {
            return ApiError::BadRequest("隧道字段不满足约束".into());
        }
    }
    ApiError::Database(e)
}
