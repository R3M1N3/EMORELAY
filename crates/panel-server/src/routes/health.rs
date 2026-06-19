use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    /// 面板版本号（workspace 版本，编译期注入，与 Agent 升级同一真相源）
    pub version: &'static str,
}

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<HealthResponse>> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    }))
}

#[cfg(test)]
mod tests {
    use super::HealthResponse;

    #[test]
    fn health_response_serializes_nonempty_version() {
        // 锁定契约：/api/health 必须输出非空 version 字段（前端设置页消费）
        let resp = HealthResponse {
            status: "ok",
            version: env!("CARGO_PKG_VERSION"),
        };
        let json = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert!(!json["version"].as_str().unwrap().is_empty());
    }
}
