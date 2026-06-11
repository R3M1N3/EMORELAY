use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("服务器内部错误")]
    Internal(#[from] anyhow::Error),

    #[error("服务器内部错误")]
    Database(#[from] sqlx::Error),

    // message 直接面向 UI 展示;机器可读类别在 ErrorBody.error 字段,不再拼前缀。
    #[error("{0}")]
    BadRequest(String),

    #[error("资源不存在")]
    NotFound,

    #[error("未登录或登录已过期")]
    Unauthorized,

    #[error("{0}")]
    UnauthorizedMsg(String),

    #[error("无权限执行此操作")]
    Forbidden,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Internal(_) | Self::Database(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Unauthorized | Self::UnauthorizedMsg(_) => {
                (StatusCode::UNAUTHORIZED, "unauthorized")
            }
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
        };
        if status.is_server_error() {
            tracing::error!(error = ?self, "server error");
        } else {
            tracing::warn!(error = ?self, "client error");
        }
        let body = ErrorBody {
            error: code,
            message: self.to_string(),
        };
        (status, Json(body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
