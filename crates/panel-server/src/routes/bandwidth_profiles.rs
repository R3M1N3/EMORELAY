use crate::{
    audit,
    auth::extractor::{ActorIp, AuthUser},
    error::{ApiError, ApiResult},
    models::bandwidth_profile::BandwidthProfile,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize)]
pub struct ProfileView {
    pub id: i64,
    pub name: String,
    pub bandwidth_mbps: i64,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<BandwidthProfile> for ProfileView {
    fn from(p: BandwidthProfile) -> Self {
        Self {
            id: p.id,
            name: p.name,
            bandwidth_mbps: p.bandwidth_mbps,
            description: p.description,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
}

#[derive(Serialize)]
pub struct ProfileListResponse {
    pub items: Vec<ProfileView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    pub bandwidth_mbps: i64,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub bandwidth_mbps: Option<i64>,
    pub description: Option<String>,
}

fn validate_mbps(n: i64) -> ApiResult<()> {
    if n > 0 {
        Ok(())
    } else {
        Err(ApiError::BadRequest("带宽必须大于 0 Mbps".into()))
    }
}

fn validate_name(n: &str) -> ApiResult<()> {
    if n.trim().is_empty() {
        Err(ApiError::BadRequest("名称不能为空".into()))
    } else {
        Ok(())
    }
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ProfileListResponse>> {
    auth.require_admin()?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let items = BandwidthProfile::list_paged(&state.pool, page_size, offset).await?;
    let total = BandwidthProfile::count(&state.pool).await?;
    Ok(Json(ProfileListResponse {
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
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    let p = BandwidthProfile::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(p.into()))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateProfileRequest>,
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    let name = req.name.trim();
    validate_name(name)?;
    validate_mbps(req.bandwidth_mbps)?;
    let new_id = BandwidthProfile::create(&state.pool, name, req.bandwidth_mbps, req.description.trim())
        .await
        .map_err(map_sqlx_to_api)?;
    let p = BandwidthProfile::find_by_id(&state.pool, new_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.create",
        Some("bandwidth_profile"),
        Some(new_id),
        Some(name),
        true,
        None,
    )
    .await;
    Ok(Json(p.into()))
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    Json(req): Json<UpdateProfileRequest>,
) -> ApiResult<Json<ProfileView>> {
    auth.require_admin()?;
    if let Some(n) = req.name.as_deref() {
        validate_name(n)?;
    }
    if let Some(m) = req.bandwidth_mbps {
        validate_mbps(m)?;
    }
    let rows = BandwidthProfile::update_fields(
        &state.pool,
        id,
        req.name.as_deref().map(str::trim),
        req.bandwidth_mbps,
        req.description.as_deref().map(str::trim),
    )
    .await
    .map_err(map_sqlx_to_api)?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    let p = BandwidthProfile::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.update",
        Some("bandwidth_profile"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    // 引用该 profile 的活跃规则即时下发新带宽(重建 Agent token bucket)。
    dispatch_referencing_rules(&state, id).await;

    Ok(Json(p.into()))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;
    let refs = BandwidthProfile::active_rule_refs(&state.pool, id).await?;
    if refs > 0 {
        return Err(ApiError::BadRequest(format!(
            "限速配置仍被 {refs} 条规则引用,请先解除关联"
        )));
    }
    let rows = BandwidthProfile::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "bandwidth_profile.delete",
        Some("bandwidth_profile"),
        Some(id),
        None,
        true,
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// profile 改动后,把引用它的活跃规则逐条 ApplyRule 重下发。
/// Agent 离线时静默跳过(下次 register reconcile 对齐)。
async fn dispatch_referencing_rules(state: &AppState, profile_id: i64) {
    use crate::models::rule::Rule;
    // best-effort:查询失败只 warn 不阻断主流程(与 audit 写失败、agent 离线分支同一约定)。
    let ids: Vec<(i64,)> = match sqlx::query_as(
        "SELECT id FROM forward_rules WHERE bandwidth_profile_id = ? AND deleted_at IS NULL",
    )
    .bind(profile_id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = ?e, profile_id, "failed to query referencing rules; skip re-dispatch");
            return;
        }
    };
    for (rule_id,) in ids {
        if let Ok(Some(rule)) = Rule::find_by_id(&state.pool, rule_id).await {
            let _ = crate::grpc::tunnel_dispatch::dispatch_rule_apply(state, &rule).await;
        }
    }
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest("限速配置名称已存在".into());
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("带宽必须大于 0 Mbps".into());
        }
    }
    ApiError::Database(e)
}
