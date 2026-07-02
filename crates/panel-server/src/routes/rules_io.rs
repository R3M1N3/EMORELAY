//! 规则导入导出(admin only)。导出不含 id/user_id/created_at(跨实例不可控);
//! 以 node_name / bandwidth_profile_name 做跨实例映射。tunnel_name 导出真实关联名
//! (供人识别),导入非空仍报 error(隧道凭据/hop 不可跨实例自动重建,需手动重建关联)。
//! P9: 导出支持 node_id/tunnel_id 过滤;导入支持 target_node_id(全部映射到指定节点)。
use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::{bandwidth_profile::BandwidthProfile, node::Node, rule::Rule, settings},
    state::AppState,
};
use crate::routes::rules::{parse_extra_targets, validate_targets, TargetDto};
use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Serialize, Deserialize)]
pub struct RuleExportItem {
    pub name: String,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: u16,
    pub target_host: String,
    pub target_port: u16,
    pub enabled: bool,
    pub node_name: String,
    pub tunnel_name: Option<String>,
    pub bandwidth_profile_name: Option<String>,
    /// 归属用户名(跨实例按用户名回填;缺失/匹配不到 → 归导入者)。老导出文件无此字段。
    #[serde(default)]
    pub owner_username: Option<String>,
    /// P2 多目标额外目标(空 = 单目标);老导出文件无此字段。
    #[serde(default)]
    pub extra_targets: Vec<TargetDto>,
    /// 负载策略;老文件无此字段默认 fifo。
    #[serde(default = "default_lb_strategy")]
    pub lb_strategy: String,
    /// 出站地址族偏好;老文件无此字段默认 auto。
    #[serde(default = "default_remote_af")]
    pub remote_af: String,
}

fn default_lb_strategy() -> String {
    "fifo".to_string()
}

fn default_remote_af() -> String {
    "auto".to_string()
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub node_id: Option<i64>,
    pub user_id: Option<i64>,
    pub tunnel_id: Option<i64>,
}

#[derive(FromRow)]
struct ExportRow {
    name: String,
    protocol: String,
    listen_ip: String,
    listen_port: i64,
    target_host: String,
    target_port: i64,
    enabled: i64,
    node_name: String,
    tunnel_name: Option<String>,
    bandwidth_profile_name: Option<String>,
    owner_username: Option<String>,
    extra_targets: Option<String>,
    lb_strategy: String,
    remote_af: String,
}

pub async fn export(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ExportQuery>,
) -> ApiResult<impl IntoResponse> {
    auth.require_admin()?;
    let mut where_parts = vec!["fr.deleted_at IS NULL".to_string()];
    if q.node_id.is_some() {
        where_parts.push("fr.node_id = ?".into());
    }
    if q.user_id.is_some() {
        where_parts.push("fr.user_id = ?".into());
    }
    if q.tunnel_id.is_some() {
        where_parts.push("fr.tunnel_id = ?".into());
    }
    let sql = format!(
        "SELECT fr.name, fr.protocol, fr.listen_ip, fr.listen_port, fr.target_host, \
                fr.target_port, fr.enabled, fr.extra_targets, fr.lb_strategy, fr.remote_af, \
                n.name AS node_name, \
                t.name AS tunnel_name, \
                bp.name AS bandwidth_profile_name, \
                u.username AS owner_username \
         FROM forward_rules fr \
         JOIN nodes n ON n.id = fr.node_id \
         LEFT JOIN tunnels t ON t.id = fr.tunnel_id AND t.deleted_at IS NULL \
         LEFT JOIN bandwidth_profiles bp \
           ON bp.id = fr.bandwidth_profile_id AND bp.deleted_at IS NULL \
         LEFT JOIN users u ON u.id = fr.user_id AND u.deleted_at IS NULL \
         WHERE {} ORDER BY fr.id",
        where_parts.join(" AND ")
    );
    let mut query = sqlx::query_as::<_, ExportRow>(&sql);
    if let Some(nid) = q.node_id {
        query = query.bind(nid);
    }
    if let Some(uid) = q.user_id {
        query = query.bind(uid);
    }
    if let Some(tid) = q.tunnel_id {
        query = query.bind(tid);
    }
    let rows = query.fetch_all(&state.pool).await?;
    let items: Vec<RuleExportItem> = rows
        .into_iter()
        .map(|r| RuleExportItem {
            name: r.name,
            protocol: r.protocol,
            listen_ip: r.listen_ip,
            listen_port: r.listen_port as u16,
            target_host: r.target_host,
            target_port: r.target_port as u16,
            enabled: r.enabled != 0,
            node_name: r.node_name,
            tunnel_name: r.tunnel_name,
            bandwidth_profile_name: r.bandwidth_profile_name,
            owner_username: r.owner_username,
            extra_targets: parse_extra_targets(r.extra_targets.as_deref()),
            lb_strategy: r.lb_strategy,
            remote_af: r.remote_af,
        })
        .collect();
    let body = serialize_export(&items)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"emorelay-rules-export.json\"",
            ),
        ],
        body,
    ))
}

/// 导出体序列化:2 空格缩进美化。抽成函数便于单测。
fn serialize_export(items: &[RuleExportItem]) -> ApiResult<String> {
    Ok(serde_json::to_string_pretty(items).map_err(anyhow::Error::from)?)
}

#[derive(Deserialize)]
pub struct ImportQuery {
    pub strategy: Option<String>,
    pub dry_run: Option<u8>,
    /// P9: 给定时忽略各 item 的 node_name,全部映射到该节点。
    pub target_node_id: Option<i64>,
}

#[derive(Serialize)]
pub struct ImportItemReport {
    pub index: usize,
    pub action: &'static str, // create | skip | overwrite | error
    pub reason: String,
}

#[derive(Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub strategy: String,
    pub items: Vec<ImportItemReport>,
}

enum PlannedAction {
    Create {
        node_id: i64,
        bandwidth_profile_id: Option<i64>,
        /// 归属:owner_username 匹配到的活跃用户;None → 归导入者。
        owner_id: Option<i64>,
        extra_json: Option<String>,
        lb_strategy: String,
    },
    Overwrite {
        existing_id: i64,
        /// 落点节点(文件内自重复检测用),与 Create 同口径。
        node_id: i64,
        bandwidth_profile_id: Option<i64>,
        extra_json: Option<String>,
        lb_strategy: String,
    },
    Skip,
    Error(String),
}

pub async fn import(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Query(q): Query<ImportQuery>,
    Json(items): Json<Vec<RuleExportItem>>,
) -> ApiResult<Json<ImportReport>> {
    auth.require_admin()?;
    // 单次导入条数上限:每项触发数次串行 DB 查询,无上限会被超大数组拖垮连接池。
    const MAX_IMPORT_ITEMS: usize = 1000;
    if items.len() > MAX_IMPORT_ITEMS {
        return Err(ApiError::BadRequest(format!(
            "单次导入最多 {MAX_IMPORT_ITEMS} 条,当前 {} 条;请拆分文件",
            items.len()
        )));
    }
    let strategy = q.strategy.as_deref().unwrap_or("skip");
    if !matches!(strategy, "skip" | "overwrite") {
        return Err(ApiError::BadRequest("导入策略必须是 skip 或 overwrite".into()));
    }
    let dry_run = q.dry_run.unwrap_or(1) != 0;
    let reserved = settings::reserved_ports(&state.pool).await;
    // 目标节点先行解析一次:不存在直接整体 400(避免逐项重复报错)。
    let target_node = match q.target_node_id {
        Some(nid) => Some(
            Node::find_by_id(&state.pool, nid)
                .await?
                .ok_or_else(|| ApiError::BadRequest("导入目标节点不存在".into()))?,
        ),
        None => None,
    };

    let mut report = Vec::with_capacity(items.len());
    let mut created = 0u32;
    let mut overwritten = 0u32;
    let mut skipped = 0u32;
    let mut errors = 0u32;

    // 文件内自重复检测:同批两项落到同一 (node, ip, port, protocol) 时,第二项直接报
    // error——否则 dry-run 各报 create、实导时却互相 skip/覆盖,预览失真(target_node_id
    // 把多节点规则汇入单节点时是主要触发场景)。
    let mut seen_bindings: std::collections::HashSet<(i64, String, u16, String)> =
        std::collections::HashSet::new();
    for (index, item) in items.iter().enumerate() {
        let mut planned = plan_item(&state, item, strategy, &reserved, target_node.as_ref()).await?;
        let landing_node = match &planned {
            PlannedAction::Create { node_id, .. } => Some(*node_id),
            PlannedAction::Overwrite { node_id, .. } => Some(*node_id),
            PlannedAction::Skip | PlannedAction::Error(_) => None,
        };
        if let Some(nid) = landing_node {
            let key = (nid, item.listen_ip.clone(), item.listen_port, item.protocol.clone());
            if !seen_bindings.insert(key) {
                planned = PlannedAction::Error("与本文件中靠前的条目监听绑定重复".into());
            }
        }
        let (action, reason): (&'static str, String) = match planned {
            PlannedAction::Error(reason) => ("error", reason),
            PlannedAction::Skip => ("skip", "相同监听绑定已存在".into()),
            PlannedAction::Create { node_id, bandwidth_profile_id, owner_id, extra_json, lb_strategy } => {
                // 归属结果进 reason,让 admin 在 dry-run 预览就能看到每条的落点归属。
                let owner_note = match (&owner_id, item.owner_username.as_deref()) {
                    (Some(_), Some(name)) => format!("归属: {name}"),
                    (None, Some(name)) if !name.is_empty() => {
                        format!("归属用户 {name} 不存在,归导入者")
                    }
                    _ => String::new(),
                };
                if dry_run {
                    ("create", owner_note)
                } else {
                    let owner = owner_id.unwrap_or(auth.0.sub);
                    match execute_create(&state, owner, item, node_id, bandwidth_profile_id, extra_json.as_deref(), &lb_strategy).await {
                        Ok(()) => ("create", owner_note),
                        Err(e) => ("error", format!("创建失败: {e}")),
                    }
                }
            }
            PlannedAction::Overwrite { existing_id, node_id: _, bandwidth_profile_id, extra_json, lb_strategy } => {
                if dry_run {
                    ("overwrite", format!("将覆盖规则 #{existing_id}"))
                } else {
                    match execute_overwrite(&state, item, existing_id, bandwidth_profile_id, extra_json.as_deref(), &lb_strategy).await {
                        Ok(()) => ("overwrite", format!("已覆盖规则 #{existing_id}")),
                        Err(e) => ("error", format!("覆盖失败: {e}")),
                    }
                }
            }
        };
        match action {
            "create" => created += 1,
            "overwrite" => overwritten += 1,
            "skip" => skipped += 1,
            _ => errors += 1,
        }
        report.push(ImportItemReport { index, action, reason });
    }

    if !dry_run {
        audit::record_with_ip(
            &state.pool,
            Some(auth.0.sub),
            actor_ip.as_option(),
            "rule.import",
            Some("rule"),
            None,
            Some(&format!(
                "strategy={strategy},created={created},overwritten={overwritten},skipped={skipped},errors={errors}"
            )),
            errors == 0,
            None,
        )
        .await;
    }

    Ok(Json(ImportReport {
        dry_run,
        strategy: strategy.to_string(),
        items: report,
    }))
}

/// 单项校验与映射,不写库。target_node 给定时忽略 item.node_name(P9 全部映射到指定节点)。
async fn plan_item(
    state: &AppState,
    item: &RuleExportItem,
    strategy: &str,
    reserved: &[i64],
    target_node: Option<&Node>,
) -> ApiResult<PlannedAction> {
    if item.tunnel_name.as_deref().is_some_and(|t| !t.is_empty()) {
        return Ok(PlannedAction::Error(
            "导入暂不支持关联隧道的规则,请导入后手动重建关联".into(),
        ));
    }
    if item.name.trim().is_empty() {
        return Ok(PlannedAction::Error("名称不能为空".into()));
    }
    if !matches!(item.protocol.as_str(), "tcp" | "udp" | "tcp_udp") {
        return Ok(PlannedAction::Error("协议必须是 tcp | udp | tcp_udp".into()));
    }
    if item.listen_port == 0 || item.target_port == 0 {
        return Ok(PlannedAction::Error("端口必须在 1-65535 之间".into()));
    }
    if !crate::util::is_valid_ip(&item.listen_ip) {
        return Ok(PlannedAction::Error("监听 IP 不是合法 IP 地址".into()));
    }
    if !crate::util::is_valid_target_host(item.target_host.trim()) {
        return Ok(PlannedAction::Error("目标主机不是合法 IP 或主机名".into()));
    }
    if !matches!(item.remote_af.as_str(), "auto" | "v4" | "v6") {
        return Ok(PlannedAction::Error("remote_af 必须是 auto | v4 | v6".into()));
    }

    let node = match target_node {
        Some(n) => n.clone(),
        None => match Node::find_by_name(&state.pool, &item.node_name).await? {
            Some(n) => n,
            None => {
                return Ok(PlannedAction::Error(format!(
                    "节点不存在: {}",
                    item.node_name
                )))
            }
        },
    };
    let port = i64::from(item.listen_port);
    if port < node.port_pool_min || port > node.port_pool_max {
        return Ok(PlannedAction::Error(format!(
            "监听端口 {} 超出节点端口池 [{}-{}]",
            port, node.port_pool_min, node.port_pool_max
        )));
    }
    if reserved.contains(&port) {
        return Ok(PlannedAction::Error(format!("监听端口 {port} 是保留端口,禁止监听")));
    }

    // profile 找不到 → NULL(不自动创建,避免误植)
    let bandwidth_profile_id = match item.bandwidth_profile_name.as_deref() {
        None | Some("") => None,
        Some(name) => BandwidthProfile::find_by_name(&state.pool, name)
            .await?
            .map(|p| p.id),
    };
    // 归属按用户名回填(P9 遗留):匹配不到/未携带 → None(归导入者),不自动建用户。
    let owner_id = match item.owner_username.as_deref() {
        None | Some("") => None,
        Some(name) => crate::models::user::User::find_by_username(&state.pool, name)
            .await?
            .map(|u| u.id),
    };

    // 多目标 + 策略校验(导入 admin only → is_admin=true,允许内网目标)。
    let (extra_json, lb_strategy) =
        match validate_targets(&item.extra_targets, Some(&item.lb_strategy), true) {
            Ok(v) => v,
            Err(ApiError::BadRequest(msg)) => return Ok(PlannedAction::Error(msg)),
            Err(e) => return Err(e),
        };

    // 冲突检测:精确同 binding → 按 strategy;互斥协议冲突 → error(那是另一条规则)。
    let exact: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND listen_port = ? AND protocol = ? \
           AND deleted_at IS NULL LIMIT 1",
    )
    .bind(node.id)
    .bind(&item.listen_ip)
    .bind(port)
    .bind(&item.protocol)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((existing_id,)) = exact {
        return Ok(match strategy {
            "overwrite" => PlannedAction::Overwrite {
                existing_id,
                node_id: node.id,
                bandwidth_profile_id,
                extra_json,
                lb_strategy,
            },
            _ => PlannedAction::Skip,
        });
    }
    let mutex_conflicts: &[&str] = match item.protocol.as_str() {
        "tcp" => &["tcp_udp"],
        "udp" => &["tcp_udp"],
        _ => &["tcp", "udp"],
    };
    let placeholders = vec!["?"; mutex_conflicts.len()].join(",");
    let sql = format!(
        "SELECT id FROM forward_rules \
         WHERE node_id = ? AND listen_ip = ? AND listen_port = ? \
           AND protocol IN ({placeholders}) AND deleted_at IS NULL LIMIT 1"
    );
    let mut mq = sqlx::query_scalar::<_, i64>(&sql)
        .bind(node.id)
        .bind(&item.listen_ip)
        .bind(port);
    for p in mutex_conflicts {
        mq = mq.bind(*p);
    }
    if mq.fetch_optional(&state.pool).await?.is_some() {
        return Ok(PlannedAction::Error(format!(
            "监听端口 {port} 与互斥协议的既有规则冲突"
        )));
    }

    Ok(PlannedAction::Create {
        node_id: node.id,
        bandwidth_profile_id,
        owner_id,
        extra_json,
        lb_strategy,
    })
}

async fn execute_create(
    state: &AppState,
    owner_id: i64,
    item: &RuleExportItem,
    node_id: i64,
    bandwidth_profile_id: Option<i64>,
    extra_json: Option<&str>,
    lb_strategy: &str,
) -> anyhow::Result<()> {
    let new_id = Rule::create(
        &state.pool,
        owner_id,
        node_id,
        item.name.trim(),
        &item.protocol,
        &item.listen_ip,
        i64::from(item.listen_port),
        item.target_host.trim(),
        i64::from(item.target_port),
        bandwidth_profile_id,
        None,
        None,
        &item.remote_af,
    )
    .await?;
    // 多目标 / 非默认策略落库(须在 dispatch 前,使下发规则包含全部目标)。
    if extra_json.is_some() || lb_strategy != "fifo" {
        Rule::set_targets(&state.pool, new_id, extra_json, lb_strategy).await?;
    }
    if !item.enabled {
        Rule::set_enabled(&state.pool, new_id, false).await?;
    }
    // per-node 锁(勿去,见 b864a64):导入 create 的 ApplyRule 须与该节点 reconcile 互斥,
    // 防 reconcile 在途时被陈旧 keep_ids 误删(导入仅非隧道规则,锁集即单节点)。
    if let Some(rule) = Rule::find_by_id(&state.pool, new_id).await? {
        crate::grpc::tunnel_dispatch::dispatch_rule_apply_locked(state, &rule).await;
    }
    Ok(())
}

async fn execute_overwrite(
    state: &AppState,
    item: &RuleExportItem,
    existing_id: i64,
    bandwidth_profile_id: Option<i64>,
    extra_json: Option<&str>,
    lb_strategy: &str,
) -> anyhow::Result<()> {
    Rule::update_fields(
        &state.pool,
        existing_id,
        Some(item.name.trim()),
        None,
        None,
        Some(item.target_host.trim()),
        Some(i64::from(item.target_port)),
        Some(bandwidth_profile_id.unwrap_or(0)),
        None,
    )
    .await?;
    Rule::set_enabled(&state.pool, existing_id, item.enabled).await?;
    // 覆盖导入:额外目标 + 策略整组替换(空 = 清空),与导出往返一致。
    Rule::set_targets(&state.pool, existing_id, extra_json, lb_strategy).await?;
    Rule::set_remote_af(&state.pool, existing_id, &item.remote_af).await?;
    // per-node 锁见 execute_create(勿去):覆盖导入 overwrite 的 ApplyRule 下发同样须互斥。
    if let Some(rule) = Rule::find_by_id(&state.pool, existing_id).await? {
        crate::grpc::tunnel_dispatch::dispatch_rule_apply_locked(state, &rule).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_export_is_pretty_and_carries_multi_target() {
        let items = vec![RuleExportItem {
            name: "r1".into(),
            protocol: "tcp".into(),
            listen_ip: "0.0.0.0".into(),
            listen_port: 100,
            target_host: "1.1.1.1".into(),
            target_port: 80,
            enabled: true,
            node_name: "n".into(),
            tunnel_name: None,
            bandwidth_profile_name: None,
            owner_username: None,
            extra_targets: vec![TargetDto { host: "2.2.2.2".into(), port: 81 }],
            lb_strategy: "round".into(),
            remote_af: "auto".into(),
        }];
        let s = serialize_export(&items).unwrap();
        assert!(s.contains("\n  "), "导出应为缩进美化 JSON: {s}");
        assert!(s.contains("\"extra_targets\""));
        assert!(s.contains("\"lb_strategy\": \"round\""));
    }
}
