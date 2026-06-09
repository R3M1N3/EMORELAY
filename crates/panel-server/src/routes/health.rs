use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<HealthResponse>> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(HealthResponse { status: "ok" }))
}
