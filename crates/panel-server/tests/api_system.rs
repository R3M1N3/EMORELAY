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
    // 测试 Config 未配 TLS,应为 false
    assert_eq!(body["grpc_tls_enabled"], false);
    assert_eq!(body["grpc_mtls_enabled"], false);
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
