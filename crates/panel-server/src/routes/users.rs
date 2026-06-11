use crate::{
    audit,
    auth::{
        extractor::{ActorIp, AuthUser},
        password::hash_password,
    },
    error::{ApiError, ApiResult},
    models::user::User,
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
pub struct UserView {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
    /// 该用户名下未软删的规则数。从 User 转换时填 0(create/update/get 路径不查聚合)。
    pub rule_count: i64,
    /// 累计 rx + tx 字节。同上,From<User> 路径填 0;list 路径走 JOIN 拿实际值。
    pub total_traffic_bytes: i64,
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
    pub period_used_bytes_cached: i64,
    pub period_used_calculated_at: Option<String>,
    /// 计算字段:max(0, limit - used);limit 为 NULL 时为 None。
    pub period_remaining_bytes: Option<i64>,
}

fn remaining(limit: Option<i64>, used: i64) -> Option<i64> {
    limit.map(|l| (l - used).max(0))
}

impl From<User> for UserView {
    fn from(u: User) -> Self {
        let period_remaining_bytes =
            remaining(u.traffic_limit_bytes_30d, u.period_used_bytes_cached);
        Self {
            id: u.id,
            username: u.username,
            role: u.role,
            created_at: u.created_at,
            updated_at: u.updated_at,
            rule_count: 0,
            total_traffic_bytes: 0,
            expires_at: u.expires_at,
            traffic_limit_bytes_30d: u.traffic_limit_bytes_30d,
            period_used_bytes_cached: u.period_used_bytes_cached,
            period_used_calculated_at: u.period_used_calculated_at,
            period_remaining_bytes,
        }
    }
}

/// 列表 SQL 投影:加上 LEFT JOIN 聚合得到的两个统计字段。
#[derive(FromRow)]
struct UserListRow {
    id: i64,
    username: String,
    role: String,
    created_at: String,
    updated_at: String,
    rule_count: i64,
    total_traffic_bytes: i64,
    expires_at: Option<String>,
    traffic_limit_bytes_30d: Option<i64>,
    period_used_bytes_cached: i64,
    period_used_calculated_at: Option<String>,
}

impl From<UserListRow> for UserView {
    fn from(r: UserListRow) -> Self {
        let period_remaining_bytes =
            remaining(r.traffic_limit_bytes_30d, r.period_used_bytes_cached);
        Self {
            id: r.id,
            username: r.username,
            role: r.role,
            created_at: r.created_at,
            updated_at: r.updated_at,
            rule_count: r.rule_count,
            total_traffic_bytes: r.total_traffic_bytes,
            expires_at: r.expires_at,
            traffic_limit_bytes_30d: r.traffic_limit_bytes_30d,
            period_used_bytes_cached: r.period_used_bytes_cached,
            period_used_calculated_at: r.period_used_calculated_at,
            period_remaining_bytes,
        }
    }
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub search: Option<String>,
}

#[derive(Serialize)]
pub struct UserListResponse {
    pub items: Vec<UserView>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: String,
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub password: Option<String>,
    pub role: Option<String>,
    /// "" = 清除
    pub expires_at: Option<String>,
    /// 0 = 清除
    pub traffic_limit_bytes_30d: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<UserListResponse>> {
    auth.require_admin()?;
    let page = q.page.unwrap_or(1).max(1);
    let page_size = q.page_size.unwrap_or(20).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let search_clause = if search.is_some() {
        " AND u.username LIKE ? ESCAPE '\\'"
    } else {
        ""
    };
    let like = search.map(|s| format!("%{}%", crate::util::escape_like(s)));

    // 一次 LEFT JOIN 拿到所有列表字段 + 规则数 + 累计流量。subquery 按 user_id
    // 预聚合(COUNT / SUM(rx+tx))避免 GROUP BY 在外层引入笛卡尔积。
    let sql = format!(
        "SELECT u.id, u.username, u.role, u.created_at, u.updated_at, \
                COALESCE(r.cnt, 0) AS rule_count, \
                COALESCE(r.tot, 0) AS total_traffic_bytes, \
                u.expires_at, u.traffic_limit_bytes_30d, \
                u.period_used_bytes_cached, u.period_used_calculated_at \
         FROM users u \
         LEFT JOIN ( \
             SELECT user_id, COUNT(*) AS cnt, SUM(rx_bytes + tx_bytes) AS tot \
             FROM forward_rules WHERE deleted_at IS NULL GROUP BY user_id \
         ) r ON r.user_id = u.id \
         WHERE u.deleted_at IS NULL{search_clause} \
         ORDER BY u.id DESC LIMIT ? OFFSET ?"
    );
    let mut rows_q = sqlx::query_as::<_, UserListRow>(&sql);
    if let Some(l) = &like {
        rows_q = rows_q.bind(l.clone());
    }
    let rows: Vec<UserListRow> = rows_q
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;

    let count_sql =
        format!("SELECT COUNT(*) FROM users u WHERE u.deleted_at IS NULL{search_clause}");
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(l) = &like {
        count_q = count_q.bind(l.clone());
    }
    let total = count_q.fetch_one(&state.pool).await?;

    Ok(Json(UserListResponse {
        items: rows.into_iter().map(Into::into).collect(),
        total,
        page,
        page_size,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<UserView>> {
    auth.require_admin()?;
    let user = User::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(user.into()))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<UserView>> {
    auth.require_admin()?;
    let username = req.username.trim();
    validate_username(username)?;
    validate_password(&req.password)?;
    validate_role(&req.role)?;
    let normalized_expires = match req.expires_at.as_deref() {
        None | Some("") => None,
        Some(s) => Some(crate::util::normalize_datetime(s).ok_or_else(|| {
            ApiError::BadRequest("到期时间格式必须是 YYYY-MM-DDTHH:MM (UTC)".into())
        })?),
    };
    if matches!(req.traffic_limit_bytes_30d, Some(n) if n < 0) {
        return Err(ApiError::BadRequest(
            "30 天流量上限不能为负数".into(),
        ));
    }
    // create 时 0 与 None 等价(都是不限)
    let limit = req.traffic_limit_bytes_30d.filter(|n| *n > 0);

    let hash = hash_password(&req.password).map_err(ApiError::Internal)?;
    let new_id = User::create(
        &state.pool,
        username,
        &hash,
        &req.role,
        normalized_expires.as_deref(),
        limit,
    )
    .await
    .map_err(map_sqlx_to_api)?;
    let user = User::find_by_id(&state.pool, new_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "user.create",
        Some("user"),
        Some(new_id),
        Some(username),
        true,
        None,
    )
    .await;

    Ok(Json(user.into()))
}

pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserView>> {
    auth.require_admin()?;
    let existing = User::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if let Some(p) = req.password.as_deref() {
        validate_password(p)?;
    }
    if let Some(r) = req.role.as_deref() {
        validate_role(r)?;
        // 自降级:JWT 直至过期仍带 admin 角色,但下一次会话拿不到管理权,
        // 与 delete 的自损保护对齐,统一拒绝(让用户请另一个 admin 操作)。
        if id == auth.0.sub && r != "admin" && existing.role == "admin" {
            return Err(ApiError::BadRequest(
                "不能降级自己的管理员权限,请由其他管理员操作".into(),
            ));
        }
        // 把当前唯一 admin 降级为 user 会让系统失去所有管理员入口。
        if existing.role == "admin" && r != "admin" {
            let others = User::count_admins_excluding(&state.pool, id).await?;
            if others == 0 {
                return Err(ApiError::BadRequest(
                    "不能降级最后一个管理员".into(),
                ));
            }
        }
    }

    // 置空协议:"" 原样传给 model 层(CASE WHEN '' THEN NULL);其余 normalize。
    let normalized_expires: Option<String> = match req.expires_at.as_deref() {
        None => None,
        Some("") => Some(String::new()),
        Some(s) => Some(crate::util::normalize_datetime(s).ok_or_else(|| {
            ApiError::BadRequest("到期时间格式必须是 YYYY-MM-DDTHH:MM (UTC)".into())
        })?),
    };
    if matches!(req.traffic_limit_bytes_30d, Some(n) if n < 0) {
        return Err(ApiError::BadRequest(
            "30 天流量上限不能为负数".into(),
        ));
    }

    let new_hash = match req.password.as_deref() {
        Some(p) => Some(hash_password(p).map_err(ApiError::Internal)?),
        None => None,
    };
    let rows = User::update(
        &state.pool,
        id,
        new_hash.as_deref(),
        req.role.as_deref(),
        normalized_expires.as_deref(),
        req.traffic_limit_bytes_30d,
    )
    .await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    let user = User::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "user.update",
        Some("user"),
        Some(id),
        None,
        true,
        None,
    )
    .await;

    Ok(Json(user.into()))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    actor_ip: ActorIp,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    auth.require_admin()?;

    // 自删:管理员把自己删了会丢失会话且无回路。
    if id == auth.0.sub {
        return Err(ApiError::BadRequest("不能删除自己的账号".into()));
    }

    let target = User::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // 删除唯一管理员会让系统失去管理入口。
    if target.role == "admin" {
        let others = User::count_admins_excluding(&state.pool, id).await?;
        if others == 0 {
            return Err(ApiError::BadRequest(
                "不能删除最后一个管理员".into(),
            ));
        }
    }

    let rows = User::soft_delete(&state.pool, id).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }

    audit::record_with_ip(
        &state.pool,
        Some(auth.0.sub),
        actor_ip.as_option(),
        "user.delete",
        Some("user"),
        Some(id),
        Some(&target.username),
        true,
        None,
    )
    .await;

    Ok(Json(json!({ "ok": true })))
}

fn validate_username(u: &str) -> ApiResult<()> {
    let len = u.chars().count();
    if !(3..=32).contains(&len) {
        return Err(ApiError::BadRequest(
            "用户名长度必须在 3-32 字符之间".into(),
        ));
    }
    if u.chars().any(char::is_whitespace) {
        return Err(ApiError::BadRequest(
            "用户名不能包含空白字符".into(),
        ));
    }
    Ok(())
}

fn validate_password(p: &str) -> ApiResult<()> {
    if p.len() < 8 {
        return Err(ApiError::BadRequest(
            "密码长度至少 8 个字符".into(),
        ));
    }
    Ok(())
}

fn validate_role(r: &str) -> ApiResult<()> {
    if matches!(r, "admin" | "user") {
        Ok(())
    } else {
        Err(ApiError::BadRequest("角色必须是 admin 或 user".into()))
    }
}

fn map_sqlx_to_api(e: sqlx::Error) -> ApiError {
    if let Some(db_err) = e.as_database_error() {
        if db_err.is_unique_violation() {
            return ApiError::BadRequest("用户名已存在".into());
        }
        if db_err.is_check_violation() {
            return ApiError::BadRequest("用户字段不满足约束".into());
        }
    }
    ApiError::Database(e)
}
