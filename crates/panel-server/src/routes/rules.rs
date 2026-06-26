use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::{
        grant,
        node::Node,
        rule::{Rule, SORT_FIELDS},
        settings,
    },
    state::AppState,
    util::{is_internal_target_ip, is_valid_ip, is_valid_target_host},
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::prelude::FromRow;

// ============= DTOs =============

/// 多目标条目(请求与视图共用)。
#[derive(Serialize, Deserialize, Clone)]
pub struct TargetDto {
    pub host: String,
    pub port: u16,
}

fn parse_extra_targets(json: Option<&str>) -> Vec<TargetDto> {
    json.and_then(|s| serde_json::from_str::<Vec<TargetDto>>(s).ok())
        .unwrap_or_default()
}

#[derive(Serialize)]
pub struct RuleView {
    pub id: i64,
    pub user_id: i64,
    /// 归属用户名(列表/详情由 routes 层补查;不过滤软删,展示软删用户的原名)。
    pub user_name: Option<String>,
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
    pub tunnel_id: Option<i64>,
    /// 并发连接上限(仅 TCP);None = 不限。
    pub max_connections: Option<i64>,
    /// P2 多目标额外目标 + 负载策略。
    pub extra_targets: Vec<TargetDto>,
    pub lb_strategy: String,
    /// 是否向上游发送 PROXY protocol v1(仅非隧道 TCP relay)。
    pub send_proxy_protocol: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Rule> for RuleView {
    fn from(r: Rule) -> Self {
        Self {
            id: r.id,
            user_id: r.user_id,
            user_name: None,
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
            tunnel_id: r.tunnel_id,
            max_connections: r.max_connections,
            extra_targets: parse_extra_targets(r.extra_targets.as_deref()),
            lb_strategy: r.lb_strategy,
            send_proxy_protocol: r.send_proxy_protocol != 0,
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
    pub user_id: Option<i64>,
    pub enabled: Option<bool>,
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
    pub tunnel_id: Option<i64>,
    /// 归属用户:仅 admin 可指定;普通用户只能为自己建(留空即可)。
    pub user_id: Option<i64>,
    /// 并发连接上限(仅 TCP)。admin 管控字段;None = 不限。
    pub max_connections: Option<i64>,
    /// P2 多目标额外目标(主目标 = target_host:target_port);空/未传 = 单目标。
    pub extra_targets: Option<Vec<TargetDto>>,
    /// 负载策略 fifo/round/rand/hash;未传 = fifo。
    pub lb_strategy: Option<String>,
    /// realm-parity:向上游发送 PROXY protocol(admin 管控);未传/false = 关。仅非隧道 TCP。
    pub send_proxy_protocol: Option<bool>,
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
    /// 0 = 清除上限(不限);admin 管控字段
    pub max_connections: Option<i64>,
    /// 给定则全量替换额外目标(空数组 = 清空回单目标);None = 不改。
    pub extra_targets: Option<Vec<TargetDto>>,
    pub lb_strategy: Option<String>,
    /// admin 管控:向上游发送 PROXY protocol 开关;None = 不改。
    pub send_proxy_protocol: Option<bool>,
}

/// 校验多目标 + 策略,返回 (extra_targets_json, lb_strategy)。每个额外目标做与主目标
/// 同等的 host 形状校验;非 admin 不得指向内网。空列表 → None(清空)。
fn validate_targets(
    extra: &[TargetDto],
    lb_strategy: Option<&str>,
    is_admin: bool,
) -> ApiResult<(Option<String>, String)> {
    let strat = lb_strategy.unwrap_or("fifo");
    if !matches!(strat, "fifo" | "round" | "rand" | "hash") {
        return Err(ApiError::BadRequest(
            "负载策略必须是 fifo | round | rand | hash".into(),
        ));
    }
    if extra.len() > 32 {
        return Err(ApiError::BadRequest("额外目标不能超过 32 个".into()));
    }
    for t in extra {
        let host = t.host.trim();
        if t.port == 0 {
            return Err(ApiError::BadRequest("目标端口必须在 1-65535 之间".into()));
        }
        if !is_valid_target_host(host) {
            return Err(ApiError::BadRequest(format!(
                "额外目标主机不合法: {host}"
            )));
        }
        if !is_admin && is_internal_target_ip(host) {
            return Err(ApiError::BadRequest(
                "额外目标不能是回环或内网地址".into(),
            ));
        }
    }
    let json = if extra.is_empty() {
        None
    } else {
        // 落库前 trim host。
        let norm: Vec<TargetDto> = extra
            .iter()
            .map(|t| TargetDto { host: t.host.trim().to_string(), port: t.port })
            .collect();
        Some(serde_json::to_string(&norm).unwrap_or_default())
    };
    Ok((json, strat.to_string()))
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
            "排序字段不合法,可用: {}",
            SORT_FIELDS.join(",")
        )));
    }
    let order_desc = match q.order.as_deref().unwrap_or("desc") {
        "asc" => false,
        "desc" => true,
        _ => return Err(ApiError::BadRequest("排序方向必须是 asc 或 desc".into())),
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
        q.user_id,
        q.enabled,
    )
    .await?;
    let total = Rule::count_filtered(
        &state.pool,
        q.node_id,
        q.protocol.as_deref(),
        q.search.as_deref(),
        restrict_user_id,
        q.user_id,
        q.enabled,
    )
    .await?;

    // 批量补归属用户名(admin 列表「归属」列用),一次 IN 查询。
    let mut views: Vec<RuleView> = items.into_iter().map(Into::into).collect();
    let mut ids: Vec<i64> = views.iter().map(|v| v.user_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if !ids.is_empty() {
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!("SELECT id, username FROM users WHERE id IN ({placeholders})");
        let mut uq = sqlx::query_as::<_, (i64, String)>(&sql);
        for id in &ids {
            uq = uq.bind(id);
        }
        let map: std::collections::HashMap<i64, String> =
            uq.fetch_all(&state.pool).await?.into_iter().collect();
        for v in &mut views {
            v.user_name = map.get(&v.user_id).cloned();
        }
    }

    Ok(Json(RuleListResponse {
        items: views,
        total,
        page,
        page_size,
    }))
}

/// 单条规则视图补归属用户名(get/create 用)。不过滤软删(FK 保证行存在,几乎恒 Some)。
async fn lookup_username(pool: &sqlx::SqlitePool, user_id: i64) -> ApiResult<Option<String>> {
    Ok(sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
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
    let mut view = RuleView::from(rule);
    view.user_name = lookup_username(&state.pool, view.user_id).await?;
    Ok(Json(view))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateRuleRequest>,
) -> ApiResult<Json<RuleView>> {
    // 归属:admin 可指定任意未删用户;普通用户只能是自己(显式传自己 id 也放行)。
    let owner_id = match req.user_id {
        Some(uid) if !auth.is_admin() => {
            if uid != auth.0.sub {
                return Err(ApiError::BadRequest("仅管理员可指定归属用户".into()));
            }
            uid
        }
        Some(uid) => {
            crate::models::user::User::find_by_id(&state.pool, uid)
                .await?
                .ok_or_else(|| ApiError::BadRequest("归属用户不存在".into()))?;
            uid
        }
        None => auth.0.sub,
    };
    // 限速档/连接数上限是 admin 管控资产,普通用户不得自配;隧道改为按授权放开(校验见下)。
    if !auth.is_admin()
        && (req.bandwidth_profile_id.is_some()
            || req.max_connections.is_some()
            || req.send_proxy_protocol.is_some())
    {
        return Err(ApiError::BadRequest(
            "仅管理员可配置限速 / 连接数上限 / PROXY protocol".into(),
        ));
    }
    // PROXY protocol 仅对非隧道 TCP relay 生效(隧道 split 恒丢弃该字段),隧道规则开启会
    // 落库 true 却永不发头=静默失效,入口直接挡住,避免管理员误解。
    if req.send_proxy_protocol == Some(true) && req.tunnel_id.is_some() {
        return Err(ApiError::BadRequest(
            "PROXY protocol 仅对非隧道 TCP 规则生效".into(),
        ));
    }
    if matches!(req.max_connections, Some(n) if n < 0) {
        return Err(ApiError::BadRequest("连接数上限不能为负数".into()));
    }

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("名称不能为空".into()));
    }
    validate_protocol(&req.protocol)?;
    if matches!(req.listen_port, Some(0)) || req.target_port == 0 {
        return Err(ApiError::BadRequest(
            "监听端口与目标端口必须在 1-65535 之间".into(),
        ));
    }
    if !is_valid_ip(&req.listen_ip) {
        return Err(ApiError::BadRequest("监听 IP 不是合法 IP 地址".into()));
    }
    if !is_valid_target_host(&req.target_host) {
        return Err(ApiError::BadRequest(
            "目标主机不是合法 IP 或主机名".into(),
        ));
    }
    // 非管理员不得把目标指向回环/内网(防借节点中转访问 Agent 机内网服务);
    // 仅拦字面 IP,域名指向内网拦不住(解析在 agent 端)。
    if !auth.is_admin() && is_internal_target_ip(req.target_host.trim()) {
        return Err(ApiError::BadRequest(
            "目标地址不能是回环或内网地址".into(),
        ));
    }
    // P2 多目标:隧道规则暂不支持;校验额外目标(同主目标规则)。
    let extra = req.extra_targets.clone().unwrap_or_default();
    if req.tunnel_id.is_some() && !extra.is_empty() {
        return Err(ApiError::BadRequest("隧道规则暂不支持多目标".into()));
    }
    let (extra_json, lb_strat) = validate_targets(&extra, req.lb_strategy.as_deref(), auth.is_admin())?;
    let node = Node::find_by_id(&state.pool, req.node_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("节点不存在".into()))?;
    // 授权校验(默认拒绝,admin 不受限):隧道规则校验隧道授权(入口节点随隧道授权,
    // 不再单独校验节点授权);普通规则校验节点授权。
    if let Some(tid) = req.tunnel_id {
        if !auth.is_admin() && !grant::tunnel_granted(&state.pool, owner_id, tid).await? {
            return Err(ApiError::BadRequest("无权使用该隧道,请联系管理员授权".into()));
        }
        // tunnel_id 给定时,node_id 必须 = 隧道入口(ordinal 0)节点。
        use crate::models::tunnel::TunnelHop;
        let hops = TunnelHop::list_for_tunnel(&state.pool, tid).await?;
        let entry = hops.iter().find(|h| h.ordinal == 0)
            .ok_or_else(|| ApiError::BadRequest("隧道不存在".into()))?;
        if entry.node_id != req.node_id {
            return Err(ApiError::BadRequest(
                "规则节点必须是隧道入口(第 1 跳)节点".into()));
        }
    } else if !auth.is_admin() && !grant::node_granted(&state.pool, owner_id, req.node_id).await? {
        return Err(ApiError::BadRequest("无权使用该节点,请联系管理员授权".into()));
    }

    // 转发条数配额(对标 flux User.num/UserTunnel.num):按 owner 判定,配额 NULL=不限
    // (admin 用户天然 NULL → 不受限)。软删规则不计入。仅创建时校验,撤销配额不影响存量。
    {
        let owner = crate::models::user::User::find_by_id(&state.pool, owner_id)
            .await?
            .ok_or_else(|| ApiError::BadRequest("归属用户不存在".into()))?;
        if let Some(limit) = owner.forward_rules_quota {
            let used: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM forward_rules WHERE user_id = ? AND deleted_at IS NULL",
            )
            .bind(owner_id)
            .fetch_one(&state.pool)
            .await?;
            if used >= limit {
                return Err(ApiError::BadRequest(format!(
                    "转发规则数已达上限({used}/{limit}),请联系管理员调整配额"
                )));
            }
        }
        // 隧道规则额外受 per-(用户,隧道) 上限约束。
        if let Some(tid) = req.tunnel_id {
            if let Some(limit) = grant::tunnel_grant_num(&state.pool, owner_id, tid).await? {
                let used: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM forward_rules \
                     WHERE user_id = ? AND tunnel_id = ? AND deleted_at IS NULL",
                )
                .bind(owner_id)
                .bind(tid)
                .fetch_one(&state.pool)
                .await?;
                if used >= limit {
                    return Err(ApiError::BadRequest(format!(
                        "该隧道内转发规则数已达上限({used}/{limit})"
                    )));
                }
            }
        }
    }
    if let Some(pid) = req.bandwidth_profile_id {
        if pid <= 0 {
            return Err(ApiError::BadRequest("限速配置 ID 必须大于 0".into()));
        }
        crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
            .await?
            .ok_or_else(|| ApiError::BadRequest("限速配置不存在".into()))?;
    }
    let reserved = settings::reserved_ports(&state.pool).await;
    let listen_port_i64 = match req.listen_port {
        // 隧道入口规则的 listen_port 是隧道 ingress 业务端口,admin 可越过节点端口池;
        // P7 起隧道授权放开给普通用户,端口池豁免仅保留给 admin(防绕开端口管控),
        // 保留端口红线对所有人生效(不能监听 22/80/443 等)。
        Some(p) if req.tunnel_id.is_some() && auth.is_admin() => {
            let p = i64::from(p);
            if reserved.contains(&p) {
                return Err(ApiError::BadRequest(format!("监听端口 {p} 是保留端口,禁止监听")));
            }
            p
        }
        Some(p) => {
            let p = i64::from(p);
            if p < node.port_pool_min || p > node.port_pool_max {
                return Err(ApiError::BadRequest(format!(
                    "监听端口 {} 超出节点端口池 [{}-{}]",
                    p, node.port_pool_min, node.port_pool_max
                )));
            }
            if reserved.contains(&p) {
                return Err(ApiError::BadRequest(format!("监听端口 {p} 是保留端口,禁止监听")));
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
        owner_id,
        req.node_id,
        name,
        &req.protocol,
        &req.listen_ip,
        listen_port_i64,
        req.target_host.trim(),
        i64::from(req.target_port),
        req.bandwidth_profile_id,
        req.tunnel_id,
        req.max_connections.filter(|n| *n > 0),
    )
    .await
    .map_err(map_sqlx_to_api)?;

    // 多目标 / 策略非默认时落库(create 取 DB 默认 NULL/fifo)。
    if extra_json.is_some() || lb_strat != "fifo" {
        Rule::set_targets(&state.pool, new_id, extra_json.as_deref(), &lb_strat).await?;
    }
    // PROXY protocol 开关(create 默认 0,仅 true 时落库)。
    if req.send_proxy_protocol == Some(true) {
        Rule::set_send_proxy_protocol(&state.pool, new_id, true).await?;
    }

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
        Some(&format!("{name},owner={owner_id}")),
        true,
        None,
    )
    .await;

    // 下发失败只 warn(实体已落库,reconcile 兜底),不让误导性 500 回给客户端。
    let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;

    let mut view = RuleView::from(rule);
    view.user_name = lookup_username(&state.pool, view.user_id).await?;
    Ok(Json(view))
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

    // 撤权后冻结存量:被撤销节点/隧道授权的普通用户不得再修改规则的目标/监听字段。
    // 否则「撤权保留存量规则」会被弱化为「撤权后仍可在该节点上任意重定向转发目标」,
    // 与 create 入口的授权校验对齐(P7 ACL 须在所有入口一致执行)。
    let redirecting = req.target_host.is_some()
        || req.target_port.is_some()
        || req.listen_ip.is_some()
        || req.listen_port.is_some()
        || req.extra_targets.is_some();
    if redirecting && !auth.is_admin() {
        let still_granted = if let Some(tid) = existing.tunnel_id {
            grant::tunnel_granted(&state.pool, existing.user_id, tid).await?
        } else {
            grant::node_granted(&state.pool, existing.user_id, existing.node_id).await?
        };
        if !still_granted {
            return Err(ApiError::BadRequest(
                "授权已撤销,无法修改该规则的目标或监听;请联系管理员".into(),
            ));
        }
    }

    if let Some(n) = req.name.as_deref() {
        if n.trim().is_empty() {
            return Err(ApiError::BadRequest("名称不能为空".into()));
        }
    }
    if let Some(ip) = req.listen_ip.as_deref() {
        if !is_valid_ip(ip) {
            return Err(ApiError::BadRequest("监听 IP 不是合法 IP 地址".into()));
        }
    }
    if let Some(host) = req.target_host.as_deref() {
        if !is_valid_target_host(host.trim()) {
            return Err(ApiError::BadRequest(
                "目标主机不是合法 IP 或主机名".into(),
            ));
        }
        if !auth.is_admin() && is_internal_target_ip(host.trim()) {
            return Err(ApiError::BadRequest(
                "目标地址不能是回环或内网地址".into(),
            ));
        }
    }
    if matches!(req.listen_port, Some(0)) || matches!(req.target_port, Some(0)) {
        return Err(ApiError::BadRequest("端口必须在 1-65535 之间".into()));
    }
    // 收紧:普通用户不得改连接数上限(否则可解除 admin 设的上限)。
    if req.max_connections.is_some() && !auth.is_admin() {
        return Err(ApiError::BadRequest("仅管理员可修改连接数上限".into()));
    }
    // 同理:PROXY protocol 开关仅 admin 可改。
    if req.send_proxy_protocol.is_some() && !auth.is_admin() {
        return Err(ApiError::BadRequest("仅管理员可修改 PROXY protocol 开关".into()));
    }
    // PROXY protocol 仅非隧道 TCP 生效:隧道规则开启属静默失效,入口挡住(同 create)。
    if req.send_proxy_protocol == Some(true) && existing.tunnel_id.is_some() {
        return Err(ApiError::BadRequest(
            "PROXY protocol 仅对非隧道 TCP 规则生效".into(),
        ));
    }
    if matches!(req.max_connections, Some(n) if n < 0) {
        return Err(ApiError::BadRequest("连接数上限不能为负数".into()));
    }
    if let Some(pid) = req.bandwidth_profile_id {
        // 收紧:普通用户不得改限速关联(否则可解除 admin 挂的限速档)。
        if !auth.is_admin() {
            return Err(ApiError::BadRequest("仅管理员可修改限速配置".into()));
        }
        if pid < 0 {
            return Err(ApiError::BadRequest("限速配置 ID 不能为负数".into()));
        }
        if pid > 0 {
            crate::models::bandwidth_profile::BandwidthProfile::find_by_id(&state.pool, pid)
                .await?
                .ok_or_else(|| ApiError::BadRequest("限速配置不存在".into()))?;
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
            "监听端口 {} 超出节点端口池 [{}-{}]",
            effective_port, node.port_pool_min, node.port_pool_max
        )));
    }
    let reserved = settings::reserved_ports(&state.pool).await;
    if reserved.contains(&effective_port) {
        return Err(ApiError::BadRequest(format!(
            "监听端口 {effective_port} 是保留端口,禁止监听"
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

    // P2 多目标:给定则校验并准备落库(写在 update_fields 后)。隧道规则不支持多目标。
    let target_update = if req.extra_targets.is_some() || req.lb_strategy.is_some() {
        let extra = req.extra_targets.clone().unwrap_or_default();
        if existing.tunnel_id.is_some() && !extra.is_empty() {
            return Err(ApiError::BadRequest("隧道规则暂不支持多目标".into()));
        }
        // lb_strategy 未给时沿用既有值(只改目标不改策略)。
        let strat = req.lb_strategy.as_deref().unwrap_or(&existing.lb_strategy);
        Some(validate_targets(&extra, Some(strat), auth.is_admin())?)
    } else {
        None
    };

    let rows = Rule::update_fields(
        &state.pool,
        id,
        req.name.as_deref().map(str::trim),
        req.listen_ip.as_deref(),
        req.listen_port.map(i64::from),
        req.target_host.as_deref().map(str::trim),
        req.target_port.map(i64::from),
        req.bandwidth_profile_id,
        req.max_connections,
    )
    .await
    .map_err(map_sqlx_to_api)?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    // 多目标变更落库(在 find_by_id 前,使下发的 proto 含新目标)。
    if let Some((extra_json, lb_strat)) = target_update {
        Rule::set_targets(&state.pool, id, extra_json.as_deref(), &lb_strat).await?;
    }
    // PROXY protocol 开关变更(None = 不改)。
    if let Some(v) = req.send_proxy_protocol {
        Rule::set_send_proxy_protocol(&state.pool, id, v).await?;
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

    // 下发失败只 warn(实体已落库,reconcile 兜底),不让误导性 500 回给客户端。
    let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;

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

    // per-node 串行锁(Gap #2):软删 + RemoveRule 下发须与该规则相关节点的 reconcile
    // 「快照读 + 重放」互斥,否则极端时序下 reconcile 可能按删除前的旧快照(keep_ids 仍含
    // 本规则)复活刚删的规则。隧道规则锁链上全部 hop 节点。锁须在软删之前取(覆盖整段),
    // 目标节点从软删前的 existing 计算(软删后隧道 hop 仍可解析)。
    let target_nodes = crate::grpc::tunnel_dispatch::rule_target_nodes(&state, &existing)
        .await
        .unwrap_or_else(|_| vec![existing.node_id]);
    let _node_guards = state.dispatcher.lock_nodes(&target_nodes).await;

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

    // 软删已成功(实体不可见);下发 RemoveRule 是 best-effort。dispatched=false 表示
    // 至少一个目标节点离线,规则在数据面可能仍在跑,将由配置对账在节点恢复后清理。
    // 复用上面已算出的 target_nodes(加锁集 == 下发集),不再二次查 tunnel_hops。
    let dispatched =
        crate::grpc::tunnel_dispatch::dispatch_rule_remove_to(&state, existing.id, &target_nodes);

    Ok(Json(json!({ "ok": true, "dispatched": dispatched })))
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
        let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(&state, &rule).await;
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
    let dispatched = crate::grpc::tunnel_dispatch::dispatch_rule_restart(&state, &rule).await?;
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
            "协议必须是 tcp | udp | tcp_udp".into(),
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
            "监听端口 {listen_port} 与该节点既有规则冲突(同端口上 tcp_udp 与 tcp/udp 互斥)"
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
    let mut blocked: std::collections::HashSet<i64> = taken
        .iter()
        .filter(|(_, proto)| conflicts(proto))
        .map(|(port, _)| *port)
        .collect();

    // 活跃隧道在本节点占用的 inter_port 一律阻塞(它绑 0.0.0.0,不分协议),
    // 与 tunnels.rs 排除 forward_rules.listen_port 对称。
    let tunnel_ports: Vec<i64> = sqlx::query_scalar(
        "SELECT th.inter_port FROM tunnel_hops th \
         JOIN tunnels t ON t.id = th.tunnel_id \
         WHERE th.node_id = ? AND th.inter_port IS NOT NULL AND t.deleted_at IS NULL",
    )
    .bind(node.id)
    .fetch_all(pool)
    .await?;
    blocked.extend(tunnel_ports);

    for port in node.port_pool_min..=node.port_pool_max {
        if !reserved.contains(&port) && !blocked.contains(&port) {
            return Ok(port);
        }
    }
    Err(ApiError::BadRequest(format!(
        "节点 {} 端口池 [{}-{}] 已无可用端口",
        node.id, node.port_pool_min, node.port_pool_max
    )))
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest(
                "相同节点/协议/监听 IP/监听端口的规则已存在".into(),
            );
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("规则字段不满足约束".into());
        }
        if db_err.is_foreign_key_violation() {
            return ApiError::BadRequest("节点或用户不存在".into());
        }
    }
    ApiError::Database(e)
}
