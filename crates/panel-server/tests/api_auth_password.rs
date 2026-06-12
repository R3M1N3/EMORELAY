mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use common::{auth_req, make_app, send};
use serde_json::{json, Value};

/// 直接打 login 路由,返回完整响应 body(用于检查 must_change_password)。
async fn login_raw(app: &Router, username: &str, password: &str) -> (StatusCode, Value) {
    let body = json!({ "username": username, "password": password });
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    send(app.clone(), req).await.unwrap()
}

#[tokio::test]
async fn admin_created_user_is_forced_to_change_password_then_cleared() {
    let app = make_app().await.unwrap();
    let admin = &app.admin_token;

    // 1) admin 新建用户 → 应被强制改密。
    let req = auth_req(
        Method::POST,
        "/api/users",
        admin,
        Some(json!({ "username": "bob", "password": "temp-pass-123", "role": "user" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 2) bob 登录:login 响应带 must_change_password=true。
    let (status, body) = login_raw(&app.app, "bob", "temp-pass-123").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["must_change_password"], true);
    let bob_token = body["token"].as_str().unwrap().to_string();

    // 3) me() 同样反映强制改密(刷新入口也挡得住)。
    let req = auth_req(Method::GET, "/api/auth/me", &bob_token, None).unwrap();
    let (_, me) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(me["must_change_password"], true);

    // 4) 旧密码错误 → 400。
    let req = auth_req(
        Method::POST,
        "/api/auth/change-password",
        &bob_token,
        Some(json!({ "old_password": "wrong-pass", "new_password": "brand-new-456" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 5) 新旧相同 → 400。
    let req = auth_req(
        Method::POST,
        "/api/auth/change-password",
        &bob_token,
        Some(json!({ "old_password": "temp-pass-123", "new_password": "temp-pass-123" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 6) 正确改密 → ok。
    let req = auth_req(
        Method::POST,
        "/api/auth/change-password",
        &bob_token,
        Some(json!({ "old_password": "temp-pass-123", "new_password": "brand-new-456" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "change failed: {body}");
    assert_eq!(body["ok"], true);

    // 7) 新密码登录:标志已清除;旧密码登录失败。
    let (status, body) = login_raw(&app.app, "bob", "brand-new-456").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["must_change_password"], false);

    let (status, _) = login_raw(&app.app, "bob", "temp-pass-123").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bootstrap_admin_is_not_forced_to_change_password() {
    let app = make_app().await.unwrap();
    // 测试夹具的 admin 由 User::create(must_change=false) 建,不应被强制。
    let (status, body) = login_raw(&app.app, "admin", "admin-test-password").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["must_change_password"], false);
}

#[tokio::test]
async fn admin_password_reset_reforces_change() {
    let app = make_app().await.unwrap();
    let admin = &app.admin_token;

    // 建用户并改密清除标志。
    let req = auth_req(
        Method::POST,
        "/api/users",
        admin,
        Some(json!({ "username": "carol", "password": "carol-pass-1", "role": "user" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let carol_id = body["id"].as_i64().unwrap();

    let (_, body) = login_raw(&app.app, "carol", "carol-pass-1").await;
    let carol_token = body["token"].as_str().unwrap().to_string();
    let req = auth_req(
        Method::POST,
        "/api/auth/change-password",
        &carol_token,
        Some(json!({ "old_password": "carol-pass-1", "new_password": "carol-self-2" })),
    )
    .unwrap();
    send(app.app.clone(), req).await.unwrap();

    // admin 重置密码 → 再次强制改密。
    let req = auth_req(
        Method::PATCH,
        &format!("/api/users/{carol_id}"),
        admin,
        Some(json!({ "password": "admin-reset-3" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let (_, body) = login_raw(&app.app, "carol", "admin-reset-3").await;
    assert_eq!(body["must_change_password"], true, "admin 重置后应再次强制改密");
}
