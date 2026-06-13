use crate::{
    audit,
    auth::{
        extractor::{ActorIp, AuthUserAllowMcp},
        jwt::encode_jwt,
        password::{dummy_hash, hash_password, verify_password},
    },
    error::{ApiError, ApiResult},
    models::user::User,
    state::AppState,
};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserView,
    /// true = 该账号被要求首登改密(admin 新建/重置后);前端据此强制跳改密页。
    pub must_change_password: bool,
}

#[derive(Serialize)]
pub struct UserView {
    pub id: i64,
    pub username: String,
    pub role: String,
}

/// /api/auth/me 的扩展视图:UserView + 配额/用量/规则聚合,
/// 供普通用户自助概览页一次拿全(login 响应保持轻量 UserView 不变)。
#[derive(Serialize)]
pub struct MeView {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub expires_at: Option<String>,
    pub traffic_limit_bytes_30d: Option<i64>,
    pub period_used_bytes_cached: i64,
    pub period_used_calculated_at: Option<String>,
    pub rule_count: i64,
    pub total_traffic_bytes: i64,
    /// 强制改密标志:刷新/重进时前端据此把用户挡在改密页(login 之外的入口)。
    pub must_change_password: bool,
}

pub async fn login(
    State(state): State<AppState>,
    actor_ip: ActorIp,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let user_opt = User::find_by_username(&state.pool, &req.username).await?;

    let user = match user_opt {
        Some(u) => u,
        None => {
            // timing oracle 防御：对未知用户也跑一次 Argon2 verify 对齐时延。
            // 返回值与错误一起吞掉——dummy_hash() 由 hash_password 生成、OnceLock
            // 缓存，几乎不可能解析失败；即使失败也必须保持与真路径相同的时延。
            let _ = verify_password(&req.password, dummy_hash());
            audit::record_with_ip(
                &state.pool,
                None,
                actor_ip.as_option(),
                "auth.login",
                Some("user"),
                None,
                Some(&req.username),
                false,
                Some("unknown_user"),
            )
            .await;
            return Err(ApiError::Unauthorized);
        }
    };

    let ok = verify_password(&req.password, &user.password_hash)
        .map_err(ApiError::Internal)?;
    if !ok {
        audit::record_with_ip(
            &state.pool,
            Some(user.id),
            actor_ip.as_option(),
            "auth.login",
            Some("user"),
            Some(user.id),
            None,
            false,
            Some("bad_password"),
        )
        .await;
        return Err(ApiError::Unauthorized);
    }

    // 账号到期拒登录(P2):normalize 后的存储格式可被 parse_sqlite_datetime 解析。
    if let Some(exp) = user.expires_at.as_deref() {
        let ts = crate::grpc::commands::parse_sqlite_datetime(exp);
        if ts > 0 && ts <= chrono::Utc::now().timestamp() {
            audit::record_with_ip(
                &state.pool,
                Some(user.id),
                actor_ip.as_option(),
                "auth.login",
                Some("user"),
                Some(user.id),
                None,
                false,
                Some("account_expired"),
            )
            .await;
            return Err(ApiError::UnauthorizedMsg("account_expired".into()));
        }
    }

    let token = encode_jwt(
        &state.config.jwt_secret,
        user.id,
        &user.username,
        &user.role,
        state.config.jwt_expiry_hours,
        user.must_change_password != 0,
    )
    .map_err(ApiError::Internal)?;

    audit::record_with_ip(
        &state.pool,
        Some(user.id),
        actor_ip.as_option(),
        "auth.login",
        Some("user"),
        Some(user.id),
        None,
        true,
        None,
    )
    .await;

    Ok(Json(LoginResponse {
        token,
        user: UserView {
            id: user.id,
            username: user.username,
            role: user.role,
        },
        must_change_password: user.must_change_password != 0,
    }))
}

pub async fn me(
    State(state): State<AppState>,
    AuthUserAllowMcp(claims): AuthUserAllowMcp,
) -> ApiResult<Json<MeView>> {
    // 单行聚合,JOIN 结构与 users::list 同构(COUNT/SUM 预聚合避免笛卡尔积)。
    type MeRow = (
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        i64,
        Option<String>,
        i64,
        i64,
        i64,
    );
    let row: Option<MeRow> = sqlx::query_as(
        "SELECT u.id, u.username, u.role, u.expires_at, u.traffic_limit_bytes_30d, \
                u.period_used_bytes_cached, u.period_used_calculated_at, \
                COALESCE(r.cnt, 0), COALESCE(r.tot, 0), u.must_change_password \
         FROM users u \
         LEFT JOIN (SELECT user_id, COUNT(*) AS cnt, SUM(rx_bytes + tx_bytes) AS tot \
                    FROM forward_rules WHERE deleted_at IS NULL GROUP BY user_id) r \
           ON r.user_id = u.id \
         WHERE u.id = ? AND u.deleted_at IS NULL",
    )
    .bind(claims.sub)
    .fetch_optional(&state.pool)
    .await?;
    let (id, username, role, expires_at, limit, used, used_at, rule_count, total, must_change) =
        row.ok_or(ApiError::Unauthorized)?;
    Ok(Json(MeView {
        id,
        username,
        role,
        expires_at,
        traffic_limit_bytes_30d: limit,
        period_used_bytes_cached: used,
        period_used_calculated_at: used_at,
        rule_count,
        total_traffic_bytes: total,
        must_change_password: must_change != 0,
    }))
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

/// 自助改密:校验旧密码 → 写入新 hash 并清除 must_change_password。
/// 任何登录用户均可调用(含首登强制改密场景);不需要 admin。
pub async fn change_password(
    State(state): State<AppState>,
    AuthUserAllowMcp(claims): AuthUserAllowMcp,
    actor_ip: ActorIp,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("新密码长度至少 8 个字符".into()));
    }
    let user = User::find_by_id(&state.pool, claims.sub)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    let ok = verify_password(&req.old_password, &user.password_hash).map_err(ApiError::Internal)?;
    if !ok {
        audit::record_with_ip(
            &state.pool,
            Some(user.id),
            actor_ip.as_option(),
            "auth.change_password",
            Some("user"),
            Some(user.id),
            None,
            false,
            Some("bad_old_password"),
        )
        .await;
        return Err(ApiError::BadRequest("当前密码不正确".into()));
    }
    if req.new_password == req.old_password {
        return Err(ApiError::BadRequest("新密码不能与当前密码相同".into()));
    }
    let new_hash = hash_password(&req.new_password).map_err(ApiError::Internal)?;
    let rows = User::change_password_self(&state.pool, user.id, &new_hash).await?;
    if rows == 0 {
        return Err(ApiError::NotFound);
    }
    audit::record_with_ip(
        &state.pool,
        Some(user.id),
        actor_ip.as_option(),
        "auth.change_password",
        Some("user"),
        Some(user.id),
        None,
        true,
        None,
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 无状态 JWT：服务器端无 session 可清，前端清掉本地 token 即注销。
/// 此端点保持 REST 形态完整，并允许未来切换 stateful session 时无需改路由。
pub async fn logout() -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({ "ok": true })))
}
