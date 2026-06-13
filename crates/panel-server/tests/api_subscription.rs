mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::{auth_req, make_app, send, send_with_headers};
use serde_json::json;

/// 取登录 token(普通用户),用于 ?token= 路径测试。
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
async fn subscription_usage_returns_userinfo_header_via_bearer() {
    let app = make_app().await.unwrap();
    let (uid, token) = common::make_user_token(&app, "subu", "password123").await.unwrap();
    // 给用户配额 + 已用量。
    sqlx::query(
        "UPDATE users SET traffic_limit_bytes_30d = 1000, period_used_bytes_cached = 250 WHERE id = ?",
    )
    .bind(uid)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let req = auth_req(Method::GET, "/api/subscription/usage", &token, None).unwrap();
    let (status, headers, body) = send_with_headers(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let ui = headers
        .get("subscription-userinfo")
        .expect("must carry Subscription-Userinfo header")
        .to_str()
        .unwrap();
    assert!(ui.contains("download=250"), "userinfo={ui}");
    assert!(ui.contains("total=1000"), "userinfo={ui}");
    assert_eq!(body["used_bytes"], 250);
    assert_eq!(body["total_bytes"], 1000);
}

#[tokio::test]
async fn subscription_usage_accepts_query_token() {
    let app = make_app().await.unwrap();
    common::make_user_token(&app, "subq", "password123").await.unwrap();
    let token = login_token(&app.app, "subq", "password123").await;

    let req = Request::get(format!("/api/subscription/usage?token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "subq");
}

#[tokio::test]
async fn subscription_usage_rejects_missing_auth() {
    let app = make_app().await.unwrap();
    let req = Request::get("/api/subscription/usage").body(Body::empty()).unwrap();
    let (status, _b) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn subscription_usage_rejects_mcp_token() {
    // 订阅端点绕过 AuthUser、自行 decode_jwt(resolve_user_id),故须独立拦 mcp token
    // (I1 补漏回归守护:与 SSE 旁路同源,bearer 与 ?token= 两条路径都应 403)。
    // admin 新建用户 → must_change_password=true,登录所得为 mcp token。
    let app = make_app().await.unwrap();
    let req = auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({ "username": "submcp", "password": "temp-pass-123", "role": "user" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let token = login_token(&app.app, "submcp", "temp-pass-123").await;

    // bearer 路径 → 403。
    let req = auth_req(Method::GET, "/api/subscription/usage", &token, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "bearer mcp token 应被订阅端点拒绝");

    // ?token= 路径 → 403(resolve_user_id 单一 chokepoint 同时拦两条路径)。
    let req = Request::get(format!("/api/subscription/usage?token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "query mcp token 应被订阅端点拒绝");
}
