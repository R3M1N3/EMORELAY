pub mod auth;
pub mod bandwidth_profiles;
pub mod health;
pub mod install;
pub mod nodes;
pub mod rules;
pub mod rules_io;
pub mod system;
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
        .route("/api/system/overview", get(system::overview))
        .route("/api/system/security", get(system::security))
        .route("/api/system/audit-logs", get(system::audit_logs))
        .route(
            "/api/system/settings",
            get(system::get_settings).patch(system::update_settings),
        )
        .merge(install_routes)
        .with_state(state)
}
