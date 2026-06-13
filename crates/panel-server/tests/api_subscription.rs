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

/// 用登录 token 换取订阅专用 token(scope=sub),订阅端点现只认它(I4)。
async fn sub_token(app: &axum::Router, login: &str) -> String {
    let req = auth_req(Method::GET, "/api/subscription/token", login, None).unwrap();
    let (status, v) = send(app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "issue sub token: {v}");
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

    // 订阅端点只认 sub token(I4):先用登录 token 换取,再 bearer 访问 usage。
    let st = sub_token(&app.app, &token).await;
    let req = auth_req(Method::GET, "/api/subscription/usage", &st, None).unwrap();
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
    // ?token= 路径同样只认 sub token(订阅客户端取不到 Authorization 头)。
    let st = sub_token(&app.app, &token).await;

    let req = Request::get(format!("/api/subscription/usage?token={st}"))
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
    // I4 后订阅端点只认 sub token(scope=="sub"),非 sub token 一律拒:mcp 登录 token
    // 的 scope 为空,自然落入「非 sub」分支被拒(bearer 与 ?token= 两条路径都应 403)。
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

#[tokio::test]
async fn sub_token_can_read_usage() {
    // sub token 正路:能访问 usage 且 Subscription-Userinfo 头正确(I4)。
    let app = make_app().await.unwrap();
    let (uid, token) = common::make_user_token(&app, "subok", "password123").await.unwrap();
    sqlx::query(
        "UPDATE users SET traffic_limit_bytes_30d = 2000, period_used_bytes_cached = 500 WHERE id = ?",
    )
    .bind(uid)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let st = sub_token(&app.app, &token).await;
    let req = auth_req(Method::GET, "/api/subscription/usage", &st, None).unwrap();
    let (status, headers, body) = send_with_headers(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let ui = headers
        .get("subscription-userinfo")
        .expect("must carry Subscription-Userinfo header")
        .to_str()
        .unwrap();
    assert!(ui.contains("download=500"), "userinfo={ui}");
    assert!(ui.contains("total=2000"), "userinfo={ui}");
    assert_eq!(body["used_bytes"], 500);
    assert_eq!(body["total_bytes"], 2000);
}

#[tokio::test]
async fn sub_token_rejected_on_business_route() {
    // sub token 只能查用量:访问业务路由(/api/rules)被 AuthUser 拒(I4)。
    let app = make_app().await.unwrap();
    let (_uid, token) = common::make_user_token(&app, "subbiz", "password123").await.unwrap();
    let st = sub_token(&app.app, &token).await;

    let req = auth_req(Method::GET, "/api/rules", &st, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "sub token 不得访问业务路由");
}

#[tokio::test]
async fn login_token_rejected_on_usage() {
    // 完整登录 token(scope="")直接访问 usage → 403:订阅端点不再接受登录 JWT(I4 核心)。
    let app = make_app().await.unwrap();
    let (_uid, token) = common::make_user_token(&app, "sublogin", "password123").await.unwrap();

    // bearer 路径。
    let req = auth_req(Method::GET, "/api/subscription/usage", &token, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "bearer 登录 token 应被订阅端点拒绝");

    // ?token= 路径。
    let req = Request::get(format!("/api/subscription/usage?token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN, "query 登录 token 应被订阅端点拒绝");
}
