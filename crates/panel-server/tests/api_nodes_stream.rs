mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{make_app, send};
use serde_json::json;
use tower::ServiceExt;

async fn login_token(app: &axum::Router, username: &str, password: &str) -> String {
    let body = json!({ "username": username, "password": password });
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let (status, v) = send(app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "login: {v}");
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn node_stream_rejects_missing_token() {
    let app = make_app().await.unwrap();
    let req = Request::get("/api/nodes/stream").body(Body::empty()).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn node_stream_rejects_non_admin() {
    let app = make_app().await.unwrap();
    common::make_user_token(&app, "ssuser", "password123").await.unwrap();
    let token = login_token(&app.app, "ssuser", "password123").await;
    let req = Request::get(format!("/api/nodes/stream?token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn node_stream_admin_opens_event_stream() {
    let app = make_app().await.unwrap();
    let token = &app.admin_token;
    let req = Request::get(format!("/api/nodes/stream?token={token}"))
        .body(Body::empty())
        .unwrap();
    // SSE 响应头在 body 之前就绪;oneshot 拿到响应即可校验,不读 body(SSE 不结束)。
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
    );
    // 发布事件不应 panic(无订阅者或有订阅者都安全)。
    app.state.publish_node_event(1);
}
