mod common;

use axum::http::{Method, Request, StatusCode};
use axum::body::Body;
use common::{auth_req, make_app, make_user_token, send};

#[tokio::test]
async fn security_info_admin_ok() {
    let app = make_app().await.unwrap();
    let req = auth_req(Method::GET, "/api/system/security", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["jwt_secret_configured"], true);
    // tests/common/mod.rs 设置的 jwt_secret 长度 = 46
    assert!(body["jwt_secret_length"].as_i64().unwrap() >= 32);
    assert_eq!(body["jwt_expiry_hours"], 24);
    // P3a:测试 Config dev_disable_mtls=true(plaintext),TLS/mTLS 都应报 false。
    assert_eq!(body["grpc_tls_enabled"], false);
    assert_eq!(body["grpc_mtls_enabled"], false);
}

#[tokio::test]
async fn security_info_reports_mtls_when_enforced() {
    // P3a:内置 CA 默认强制 mTLS。这里克隆 make_app 的 state、把 dev_disable_mtls 翻成 false,
    // 复用同一 pool/jwt_secret 重建一个 router(无需重新 bootstrap CA / DB / admin),
    // 验证 security 端点据此报 TLS=mTLS=true。
    let app = make_app().await.unwrap();
    let mut state = app.state.clone();
    state.config.dev_disable_mtls = false;
    let enforced_app = panel_server::routes::router(state);

    let req = auth_req(Method::GET, "/api/system/security", &app.admin_token, None).unwrap();
    let (status, body) = send(enforced_app, req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["grpc_tls_enabled"], true);
    assert_eq!(body["grpc_mtls_enabled"], true);
}

#[tokio::test]
async fn security_info_requires_auth() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/system/security")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn security_info_rejects_non_admin() {
    let app = make_app().await.unwrap();
    let (_, user_token) = make_user_token(&app, "alice", "alice12345").await.unwrap();
    let req = auth_req(Method::GET, "/api/system/security", &user_token, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn agent_control_endpoint_accepts_https() {
    let app = make_app().await.unwrap();
    let req = auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "https://relay.example.com:50051" }
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn agent_control_endpoint_rejects_bad_scheme() {
    let app = make_app().await.unwrap();
    let req = auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "ftp://x.com" }
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn agent_control_endpoint_empty_accepted() {
    let app = make_app().await.unwrap();
    let req = auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "" }
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

// ============ P4: overview 24h 转发流量口径 ============

#[tokio::test]
async fn overview_includes_24h_forward_traffic() {
    let app = make_app().await.unwrap();
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('ovn', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
         VALUES (?, 1, 'ovr', 'tcp', '0.0.0.0', 21001, '1.2.3.4', 443)",
    )
    .bind(app.admin_user_id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    // 1h 前(窗口内)与 25h 前(窗口外)各一桶
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) \
         VALUES (1, datetime('now','-1 hour'), 100, 200), (1, datetime('now','-25 hours'), 999, 999)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let req = auth_req(Method::GET, "/api/system/overview", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rx_bytes_24h"], 100, "{body}");
    assert_eq!(body["tx_bytes_24h"], 200, "{body}");
}
