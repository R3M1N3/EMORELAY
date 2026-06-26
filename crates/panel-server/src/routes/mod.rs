pub mod auth;
pub mod bandwidth_profiles;
pub mod diagnose;
pub mod health;
pub mod install;
pub mod nodes;
pub mod rules;
pub mod rules_io;
pub mod subscription;
pub mod system;
pub mod tunnels;
pub mod users;

use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use std::time::Duration;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::KeyExtractor,
    key_extractor::SmartIpKeyExtractor, GovernorLayer,
};

/// 按已认证 user 的 id 限流的 key extractor:从 `Authorization: Bearer <jwt>` 解出
/// `claims.sub` 作为 key。diagnose 端点用它防止任意认证用户高频诊断把 probe_waiters
/// 撑爆(隧道按跳数 fan-out 放大)。
///
/// 解不出(无/坏 token)时回落到 key 0:让本层放行、把鉴权交还给 handler 的 `AuthUser`
/// 提取器(返回 401/403),避免限流层抢先吞掉鉴权语义。匿名请求共享 key 0 桶——它们
/// 本就会在 handler 处被拒,这里的限流只为已认证用户按真实 id 隔离。
#[derive(Clone)]
struct UserIdKeyExtractor {
    jwt_secret: Arc<str>,
}

impl KeyExtractor for UserIdKeyExtractor {
    type Key = i64;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::errors::GovernorError> {
        let sub = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|raw| raw.strip_prefix("Bearer "))
            .and_then(|token| crate::auth::jwt::decode_jwt(&self.jwt_secret, token).ok())
            .map(|claims| claims.sub)
            .unwrap_or(0);
        Ok(sub)
    }
}

/// governor 默认 429 是 text/plain 英文,违背全站统一 JSON 错误格式;统一成与 ApiError
/// 同构的 body 并带 Retry-After。429 文案参数化(各限流入口语境不同),作为 error_handler
/// 工厂复用——login / diagnose 共用同一实现,仅传入各自文案的 `{wait_time}` 模板。
///
/// `message`:接收剩余等待秒数(u64),返回该入口的 429 文案。
fn governor_json_error<F>(message: F) -> impl Fn(tower_governor::errors::GovernorError) -> axum::response::Response + Clone
where
    F: Fn(u64) -> String + Clone,
{
    move |err| match err {
        tower_governor::errors::GovernorError::TooManyRequests { wait_time, .. } => {
            let body = serde_json::json!({
                "error": "too_many_requests",
                "message": message(wait_time),
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
    }
}

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
            GovernorLayer::new(login_governor).error_handler(governor_json_error(|wait_time| {
                format!("尝试过于频繁,请在 {wait_time} 秒后重试")
            })),
        )
        .with_state(state.clone());

    // diagnose 防滥用:per-user(claims.sub)稳态 1 次/2s、突发 3 次。隧道诊断按跳数
    // fan-out 放大 probe,无上限时单用户即可堆爆 probe_waiters。与 state 的 64 上限
    // 形成纵深(本层挡高频,上限挡瞬时并发)。
    let diagnose_governor = Arc::new(
        GovernorConfigBuilder::default()
            .period(Duration::from_secs(2))
            .burst_size(3)
            .key_extractor(UserIdKeyExtractor {
                jwt_secret: Arc::from(state.config.jwt_secret.as_str()),
            })
            .finish()
            .expect("diagnose governor config"),
    );
    let diagnose_routes = Router::new()
        .route("/api/rules/{id}/diagnose", post(diagnose::diagnose_rule))
        .route("/api/tunnels/{id}/diagnose", post(diagnose::diagnose_tunnel))
        .layer(GovernorLayer::new(diagnose_governor).error_handler(governor_json_error(
            |wait_time| format!("诊断过于频繁，请在 {wait_time} 秒后重试"),
        )))
        .with_state(state.clone());

    Router::new()
        .route("/api/health", get(health::health))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/change-password", post(auth::change_password))
        .route("/api/subscription/usage", get(subscription::usage))
        .route("/api/subscription/token", get(subscription::issue_token))
        .route("/api/nodes", get(nodes::list).post(nodes::create))
        .route("/api/nodes/stream", get(nodes::stream))
        .route(
            "/api/nodes/{id}",
            get(nodes::get).patch(nodes::update).delete(nodes::delete),
        )
        .route("/api/nodes/{id}/stats", get(nodes::stats))
        .route("/api/nodes/{id}/grants", get(nodes::grants))
        .route("/api/nodes/{id}/upgrade-agent", post(nodes::upgrade_agent))
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
        .route("/api/users/{id}/grants", get(users::grants))
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
        .route("/api/tunnels/{id}/grants", get(tunnels::grants))
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
        .merge(diagnose_routes)
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
