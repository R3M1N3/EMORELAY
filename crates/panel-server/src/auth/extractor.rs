use crate::{
    auth::jwt::{decode_jwt, Claims},
    error::ApiError,
    state::AppState,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::header,
    http::request::Parts,
};
use std::net::SocketAddr;

/// 从 Authorization: Bearer <token> 解析 JWT；失败一律 401。
pub struct AuthUser(pub Claims);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthorized)?;
        let token = raw.strip_prefix("Bearer ").ok_or(ApiError::Unauthorized)?;
        let claims =
            decode_jwt(&state.config.jwt_secret, token).map_err(|_| ApiError::Unauthorized)?;
        if claims.mcp {
            // 强制改密未完成:除 me/change-password 外一律拒绝(服务端 enforcement,I1)。
            return Err(ApiError::Forbidden);
        }
        if claims.scope == "sub" {
            // 订阅专用 token 仅限用量端点,不得访问其它路由(I4)。
            return Err(ApiError::Forbidden);
        }
        Ok(AuthUser(claims))
    }
}

/// 与 AuthUser 同,但允许 mcp(强制改密)token——仅供 me / change-password 使用。
pub struct AuthUserAllowMcp(pub Claims);

impl FromRequestParts<AppState> for AuthUserAllowMcp {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthorized)?;
        let token = raw.strip_prefix("Bearer ").ok_or(ApiError::Unauthorized)?;
        let claims =
            decode_jwt(&state.config.jwt_secret, token).map_err(|_| ApiError::Unauthorized)?;
        if claims.scope == "sub" {
            // 订阅专用 token 仅限用量端点,不得访问其它路由(I4)。
            return Err(ApiError::Forbidden);
        }
        Ok(AuthUserAllowMcp(claims))
    }
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.0.role == "admin"
    }

    pub fn require_admin(&self) -> Result<(), ApiError> {
        if self.is_admin() {
            Ok(())
        } else {
            Err(ApiError::Forbidden)
        }
    }
}

/// 提取客户端 IP 用于 audit_logs.actor_ip。
/// 顺序:X-Real-IP → X-Forwarded-For 第一个 → tcp 对端。
/// 反代后真实 IP 必定在 X-Real-IP / X-Forwarded-For;直连时退到 ConnectInfo。
/// 任何情况下都成功(Infallible),提不到则空串。
pub struct ActorIp(pub String);

impl ActorIp {
    /// 空串 → None,方便直接传给 `audit::record_with_ip`(让 actor_ip 列保留 NULL)。
    pub fn as_option(&self) -> Option<&str> {
        if self.0.is_empty() {
            None
        } else {
            Some(&self.0)
        }
    }
}

impl FromRequestParts<AppState> for ActorIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(v) = parts
            .headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
        {
            return Ok(ActorIp(v.trim().to_string()));
        }
        if let Some(v) = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            // 多跳反代时 X-Forwarded-For 是 "client, proxy1, proxy2",取最左 client。
            let first = v.split(',').next().unwrap_or(v).trim();
            return Ok(ActorIp(first.to_string()));
        }
        if let Some(ConnectInfo(addr)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
            return Ok(ActorIp(addr.ip().to_string()));
        }
        Ok(ActorIp(String::new()))
    }
}
