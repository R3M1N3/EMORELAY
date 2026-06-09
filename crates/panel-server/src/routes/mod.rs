pub mod auth;
pub mod health;
pub mod install;
pub mod nodes;
pub mod rules;
pub mod system;
pub mod users;

use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/nodes", get(nodes::list).post(nodes::create))
        .route(
            "/api/nodes/{id}",
            get(nodes::get).patch(nodes::update).delete(nodes::delete),
        )
        .route("/api/nodes/{id}/stats", get(nodes::stats))
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
        .route("/api/users", get(users::list).post(users::create))
        .route(
            "/api/users/{id}",
            get(users::get).patch(users::update).delete(users::delete),
        )
        .route("/api/system/overview", get(system::overview))
        .route("/api/system/security", get(system::security))
        .route("/api/system/audit-logs", get(system::audit_logs))
        .route(
            "/api/system/settings",
            get(system::get_settings).patch(system::update_settings),
        )
        .route("/install.sh", get(install::install_sh))
        .route("/dist/{filename}", get(install::dist_binary))
        .with_state(state)
}
