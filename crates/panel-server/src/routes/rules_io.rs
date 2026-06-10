//! 规则导入导出(admin only)。导出不含 id/user_id/created_at(跨实例不可控);
//! 以 node_name / bandwidth_profile_name 做跨实例映射。tunnel_name 字段为
//! P3 预留:导出恒 null,导入非空报 error。
use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    grpc::commands::apply_command,
    models::{bandwidth_profile::BandwidthProfile, node::Node, rule::Rule, settings},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

#[derive(Serialize, Deserialize, FromRow)]
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
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub node_id: Option<i64>,
    pub user_id: Option<i64>,
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
    bandwidth_profile_name: Option<String>,
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
    let sql = format!(
        "SELECT fr.name, fr.protocol, fr.listen_ip, fr.listen_port, fr.target_host, \
                fr.target_port, fr.enabled, n.name AS node_name, \
                bp.name AS bandwidth_profile_name \
         FROM forward_rules fr \
         JOIN nodes n ON n.id = fr.node_id \
         LEFT JOIN bandwidth_profiles bp \
           ON bp.id = fr.bandwidth_profile_id AND bp.deleted_at IS NULL \
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
            tunnel_name: None,
            bandwidth_profile_name: r.bandwidth_profile_name,
        })
        .collect();
    Ok((
        [(
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"emorelay-rules-export.json\"",
        )],
        Json(items),
    ))
}

#[derive(Deserialize)]
pub struct ImportQuery {
    pub strategy: Option<String>,
    pub dry_run: Option<u8>,
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
    },
    Overwrite {
        existing_id: i64,
        bandwidth_profile_id: Option<i64>,
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
    let strategy = q.strategy.as_deref().unwrap_or("skip");
    if !matches!(strategy, "skip" | "overwrite") {
        return Err(ApiError::BadRequest("strategy must be skip | overwrite".into()));
    }
    let dry_run = q.dry_run.unwrap_or(1) != 0;
    let reserved = settings::reserved_ports(&state.pool).await;

    let mut report = Vec::with_capacity(items.len());
    let mut created = 0u32;
    let mut overwritten = 0u32;
    let mut skipped = 0u32;
    let mut errors = 0u32;

    for (index, item) in items.iter().enumerate() {
        let planned = plan_item(&state, item, strategy, &reserved).await?;
        let (action, reason): (&'static str, String) = match planned {
            PlannedAction::Error(reason) => ("error", reason),
            PlannedAction::Skip => ("skip", "binding already exists".into()),
            PlannedAction::Create { node_id, bandwidth_profile_id } => {
                if dry_run {
                    ("create", String::new())
                } else {
                    match execute_create(&state, &auth, item, node_id, bandwidth_profile_id).await {
                        Ok(()) => ("create", String::new()),
                        Err(e) => ("error", format!("create failed: {e}")),
                    }
                }
            }
            PlannedAction::Overwrite { existing_id, bandwidth_profile_id } => {
                if dry_run {
                    ("overwrite", format!("will patch rule #{existing_id}"))
                } else {
                    match execute_overwrite(&state, item, existing_id, bandwidth_profile_id).await {
                        Ok(()) => ("overwrite", format!("patched rule #{existing_id}")),
                        Err(e) => ("error", format!("overwrite failed: {e}")),
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

/// 单项校验与映射,不写库。
async fn plan_item(
    state: &AppState,
    item: &RuleExportItem,
    strategy: &str,
    reserved: &[i64],
) -> ApiResult<PlannedAction> {
    if item.tunnel_name.as_deref().is_some_and(|t| !t.is_empty()) {
        return Ok(PlannedAction::Error(
            "tunnel feature unavailable until P3".into(),
        ));
    }
    if item.name.trim().is_empty() {
        return Ok(PlannedAction::Error("name is required".into()));
    }
    if !matches!(item.protocol.as_str(), "tcp" | "udp" | "tcp_udp") {
        return Ok(PlannedAction::Error("protocol must be tcp | udp | tcp_udp".into()));
    }
    if item.listen_port == 0 || item.target_port == 0 {
        return Ok(PlannedAction::Error("ports must be 1-65535".into()));
    }
    if !crate::util::is_valid_ip(&item.listen_ip) {
        return Ok(PlannedAction::Error("listen_ip is not a valid IP".into()));
    }
    if !crate::util::is_valid_target_host(item.target_host.trim()) {
        return Ok(PlannedAction::Error("target_host is not a valid IP or hostname".into()));
    }

    let Some(node) = Node::find_by_name(&state.pool, &item.node_name).await? else {
        return Ok(PlannedAction::Error(format!(
            "node not found: {}",
            item.node_name
        )));
    };
    let port = i64::from(item.listen_port);
    if port < node.port_pool_min || port > node.port_pool_max {
        return Ok(PlannedAction::Error(format!(
            "listen_port {} outside node's port pool [{}-{}]",
            port, node.port_pool_min, node.port_pool_max
        )));
    }
    if reserved.contains(&port) {
        return Ok(PlannedAction::Error(format!("listen_port {port} is reserved")));
    }

    // profile 找不到 → NULL(不自动创建,避免误植)
    let bandwidth_profile_id = match item.bandwidth_profile_name.as_deref() {
        None | Some("") => None,
        Some(name) => BandwidthProfile::find_by_name(&state.pool, name)
            .await?
            .map(|p| p.id),
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
            "overwrite" => PlannedAction::Overwrite { existing_id, bandwidth_profile_id },
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
            "listen_port {port} conflicts with an existing rule of a mutually-exclusive protocol"
        )));
    }

    Ok(PlannedAction::Create {
        node_id: node.id,
        bandwidth_profile_id,
    })
}

async fn execute_create(
    state: &AppState,
    auth: &AuthUser,
    item: &RuleExportItem,
    node_id: i64,
    bandwidth_profile_id: Option<i64>,
) -> anyhow::Result<()> {
    let new_id = Rule::create(
        &state.pool,
        auth.0.sub,
        node_id,
        item.name.trim(),
        &item.protocol,
        &item.listen_ip,
        i64::from(item.listen_port),
        item.target_host.trim(),
        i64::from(item.target_port),
        bandwidth_profile_id,
    )
    .await?;
    if !item.enabled {
        Rule::set_enabled(&state.pool, new_id, false).await?;
    }
    if let Some(rule) = Rule::find_by_id(&state.pool, new_id).await? {
        if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
            tracing::warn!(node_id = rule.node_id, rule_id = new_id, "agent offline; imported rule syncs at next register");
        }
    }
    Ok(())
}

async fn execute_overwrite(
    state: &AppState,
    item: &RuleExportItem,
    existing_id: i64,
    bandwidth_profile_id: Option<i64>,
) -> anyhow::Result<()> {
    Rule::update_fields(
        &state.pool,
        existing_id,
        Some(item.name.trim()),
        None,
        None,
        Some(item.target_host.trim()),
        Some(i64::from(item.target_port)),
        // None=不改;导入 profile 缺失映射为解除关联(0)
        Some(bandwidth_profile_id.unwrap_or(0)),
    )
    .await?;
    Rule::set_enabled(&state.pool, existing_id, item.enabled).await?;
    if let Some(rule) = Rule::find_by_id(&state.pool, existing_id).await? {
        if !state.dispatcher.dispatch(rule.node_id, apply_command(&rule)) {
            tracing::warn!(node_id = rule.node_id, rule_id = existing_id, "agent offline; overwrite syncs at next register");
        }
    }
    Ok(())
}
