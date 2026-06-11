pub mod auth;
pub mod bandwidth_profiles;
pub mod health;
pub mod install;
pub mod nodes;
pub mod rules;
pub mod rules_io;
pub mod system;
pub mod tunnels;
pub mod users;

use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};

pub fn router(state: AppState) -> Router {
    let install_governor = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(1)
            .burst_size(60)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("install governor config"),
    );

    let install_routes = Router::new()
        .route("/install.sh", get(install::install_sh))
        .route("/dist/{filename}", get(install::dist_binary))
        .layer(GovernorLayer::new(install_governor))
        .with_state(state.clone());

    // 登录防爆破:稳态 1 次/秒、突发 10 次(per-IP),对正常用户无感。
    let login_governor = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(1)
            .burst_size(10)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("login governor config"),
    );
    let login_routes = Router::new()
        .route("/api/auth/login", post(auth::login))
        .layer(
            GovernorLayer::new(login_governor)
                // 默认 429 是 text/plain 英文,违背全站统一 JSON 错误格式;改成与
                // ApiError 同构的 body 并带 Retry-After。
                .error_handler(|err| match err {
                    tower_governor::errors::GovernorError::TooManyRequests {
                        wait_time, ..
                    } => {
                        let body = serde_json::json!({
                            "error": "too_many_requests",
                            "message": format!("尝试过于频繁,请在 {wait_time} 秒后重试"),
                        });
                        axum::response::Response::builder()
                            .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
                            .header("content-type", "application/json")
                            .header("retry-after", wait_time.to_string())
                            .body(axum::body::Body::from(body.to_string()))
                            .expect("static 429 response")
                    }
                    _ => axum::response::Response::builder()
                        .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"error":"internal_error","message":"服务器内部错误"}"#,
                        ))
                        .expect("static 500 response"),
                }),
        )
        .with_state(state.clone());

    Router::new()
        .route("/api/health", get(health::health))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/api/nodes/{id}",
            get(nodes::get).patch(nodes::update).delete(nodes::delete),
        )
        .route("/api/nodes/{id}/stats", get(nodes::stats))
        .route(
            "/api/nodes/{id}/revoke-credentials",
            post(nodes::revoke_credentials),
        )
        .route("/api/rules", get(rules::list).post(rules::create))
        .route(
            "/api/rules/{id}",
            get(rules::get).patch(rules::update).delete(rules::delete),
        )
        .route("/api/rules/{id}/enable", post(rules::enable))
        .route("/api/rules/{id}/disable", post(rules::disable))
        .route("/api/rules/{id}/restart", post(rules::restart))
        .route("/api/rules/{id}/stats", get(rules::stats))
        .route("/api/rules/{id}/logs", get(rules::logs))
        .route("/api/rules/export", get(rules_io::export))
        .route("/api/rules/import", post(rules_io::import))
        .route("/api/users", get(users::list).post(users::create))
        .route(
            "/api/users/{id}",
            get(users::get).patch(users::update).delete(users::delete),
        )
        .route(
            "/api/bandwidth-profiles",
            get(bandwidth_profiles::list).post(bandwidth_profiles::create),
        )
        .route(
            "/api/bandwidth-profiles/{id}",
            get(bandwidth_profiles::get)
                .patch(bandwidth_profiles::update)
                .delete(bandwidth_profiles::delete),
        )
        .route("/api/tunnels", get(tunnels::list).post(tunnels::create))
        .route(
            "/api/tunnels/{id}",
            get(tunnels::get).patch(tunnels::update).delete(tunnels::delete),
        )
        .route("/api/tunnels/{id}/restart", post(tunnels::restart))
        .route("/api/tunnels/{id}/status", get(tunnels::status))
        .route("/api/ui-config", get(system::ui_config))
        .route("/api/system/overview", get(system::overview))
        .route("/api/system/security", get(system::security))
        .route("/api/system/audit-logs", get(system::audit_logs))
        .route(
            "/api/system/settings",
            get(system::get_settings).patch(system::update_settings),
        )
        .merge(install_routes)
        .merge(login_routes)
        .with_state(state)
}

#[cfg(test)]
mod governor_probe {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// 最小复现:governor 在 oneshot + x-forwarded-for 下是否真的限流。
    #[tokio::test]
    async fn governor_limits_after_burst() {
        let cfg = Arc::new(
            GovernorConfigBuilder::default()
                .per_second(1)
                .burst_size(2)
                .key_extractor(SmartIpKeyExtractor)
                .finish()
                .unwrap(),
        );
        // 模拟真实结构:子 Router 带 layer + with_state(()),merge 进主 Router。
        let limited: axum::Router = axum::Router::new()
            .route("/x", get(|| async { "ok" }))
            .layer(GovernorLayer::new(cfg))
            .with_state(());
        let app: axum::Router = axum::Router::new()
            .route("/y", get(|| async { "ok" }))
            .merge(limited);
        let mut statuses = Vec::new();
        for _ in 0..4 {
            let req = Request::get("/x")
                .header("x-forwarded-for", "9.9.9.9")
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            statuses.push(resp.status().as_u16());
        }
        assert_eq!(statuses, vec![200, 200, 429, 429], "burst 2 后应 429");
    }
}
