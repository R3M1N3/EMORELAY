//! 订阅用量披露(对标 flux open_api/sub_store)。**只读披露配额用量,绝不分发
//! 节点/代理配置**——守住 CLAUDE.md「范围外:订阅」红线(不做订阅分发,只回用量)。
//! 返回 Clash 风格 `Subscription-Userinfo` 头,让用户在客户端直接看到套餐余量。
//! 鉴权:Authorization: Bearer <jwt>(同站)或 ?token=<jwt>(订阅客户端取不了 header)。
use crate::{
    auth::extractor::AuthUser,
    auth::jwt::decode_jwt,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct SubQuery {
    pub token: Option<String>,
}

/// 解析鉴权:优先 Authorization: Bearer,回落 ?token= 查询参数。返回 user_id。
fn resolve_user_id(state: &AppState, headers: &HeaderMap, q: &SubQuery) -> ApiResult<i64> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_string)
        .or_else(|| q.token.clone())
        .ok_or(ApiError::Unauthorized)?;
    let claims = decode_jwt(&state.config.jwt_secret, &token).map_err(|_| ApiError::Unauthorized)?;
    if claims.scope != "sub" {
        // 订阅端点只认订阅专用 token(I4):不再接受完整登录 JWT。
        return Err(ApiError::Forbidden);
    }
    Ok(claims.sub)
}

/// 为当前登录用户签发订阅专用 token(scope=sub,到账号有效期;无到期则远期)。
/// 前端订阅链接展示用,替代把完整登录 JWT 放进 URL(I4)。可重复调用(非一次性)。
pub async fn issue_token(
    State(state): State<AppState>,
    AuthUser(claims): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let expires_at: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT expires_at FROM users WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(claims.sub)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    // exp = 账号到期;无到期(parse 返回 0)则远期 10 年。
    let exp = expires_at
        .as_deref()
        .map(crate::grpc::commands::parse_sqlite_datetime)
        .filter(|ts| *ts > 0)
        .unwrap_or_else(|| chrono::Utc::now().timestamp() + 10 * 365 * 24 * 3600);
    let token = crate::auth::jwt::encode_sub_token(
        &state.config.jwt_secret,
        claims.sub,
        &claims.username,
        exp,
    )
    .map_err(ApiError::Internal)?;
    Ok(Json(json!({ "token": token, "expire_unix": exp })))
}

pub async fn usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<SubQuery>,
) -> ApiResult<impl IntoResponse> {
    let user_id = resolve_user_id(&state, &headers, &q)?;

    // 只查用量相关字段,不触碰任何规则/节点配置。
    let row: Option<(String, Option<i64>, i64, Option<String>)> = sqlx::query_as(
        "SELECT username, traffic_limit_bytes_30d, period_used_bytes_cached, expires_at \
         FROM users WHERE id = ? AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let (username, limit, used, expires_at) = row.ok_or(ApiError::Unauthorized)?;

    let total = limit.unwrap_or(0).max(0);
    let used = used.max(0);
    // expire: 到期 unix 秒;无到期 = 0(客户端约定 0 表示不过期)。
    let expire = expires_at
        .as_deref()
        .map(crate::grpc::commands::parse_sqlite_datetime)
        .filter(|ts| *ts > 0)
        .unwrap_or(0);

    // Clash/sub-store 约定:download+upload = 已用;我们不拆方向,全部记 download。
    let userinfo = format!("upload=0; download={used}; total={total}; expire={expire}");
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "subscription-userinfo",
        userinfo.parse().expect("ascii header value"),
    );

    let body = Json(json!({
        "username": username,
        "used_bytes": used,
        "total_bytes": total,
        "expire_unix": expire,
    }));
    Ok((resp_headers, body))
}
