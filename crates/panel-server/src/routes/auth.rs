use crate::{
    audit,
    auth::{
        extractor::{ActorIp, AuthUser},
        jwt::encode_jwt,
        password::{dummy_hash, verify_password},
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
}

#[derive(Serialize)]
pub struct UserView {
    pub id: i64,
    pub username: String,
    pub role: String,
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

    let token = encode_jwt(
        &state.config.jwt_secret,
        user.id,
        &user.username,
        &user.role,
        state.config.jwt_expiry_hours,
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
    }))
}

pub async fn me(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> ApiResult<Json<UserView>> {
    let user = User::find_by_id(&state.pool, claims.sub)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(UserView {
        id: user.id,
        username: user.username,
        role: user.role,
    }))
}

/// 无状态 JWT：服务器端无 session 可清，前端清掉本地 token 即注销。
/// 此端点存在让 API 形态与 plan.md 第七节对齐，并允许未来切换 stateful session 时无需改路由。
pub async fn logout() -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(json!({ "ok": true })))
}
