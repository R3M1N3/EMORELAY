mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;

#[tokio::test]
async fn login_then_me_returns_admin() {
    let app = make_app().await.unwrap();
    let req = auth_req(Method::GET, "/api/auth/me", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
    assert_eq!(body["id"], app.admin_user_id);
}

#[tokio::test]
async fn login_with_bad_password_returns_401() {
    let app = make_app().await.unwrap();
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": "admin", "password": "wrong" })).unwrap(),
        ))
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_unknown_user_returns_401_and_dummy_hash_runs() {
    let app = make_app().await.unwrap();
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": "nobody", "password": "any" })).unwrap(),
        ))
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_without_token_returns_401() {
    let app = make_app().await.unwrap();
    let req = Request::get("/api/auth/me").body(Body::empty()).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_bad_token_returns_401() {
    let app = make_app().await.unwrap();
    let req = Request::get("/api/auth/me")
        .header("authorization", "Bearer not-a-real-jwt")
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rate_limited_after_burst() {
    let app = make_app().await.unwrap();
    // 同一 IP 连续打。注意:每次失败登录都跑 Argon2(~几百 ms),期间 governor 以 1/s
    // 回填配额,所以余量 = burst 10 + 经过秒数;30 次循环在第 ~15 次后必触发 429。
    let mut last = StatusCode::OK;
    let mut last_body = serde_json::Value::Null;
    for _ in 0..30 {
        let req = Request::post("/api/auth/login")
            .header("content-type", "application/json")
            .header("x-forwarded-for", "10.9.9.9")
            .body(Body::from(
                serde_json::to_vec(&json!({ "username": "nobody", "password": "wrong-password" }))
                    .unwrap(),
            ))
            .unwrap();
        let (status, body) = send(app.app.clone(), req).await.unwrap();
        last = status;
        last_body = body;
        if status == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS, "突发额度耗尽后应触发限速");
    // 429 必须仍是统一 JSON 错误格式(红线),而非 governor 默认 text/plain。
    assert_eq!(last_body["error"], "too_many_requests", "{last_body}");
    assert!(last_body["message"].as_str().unwrap().contains("频繁"), "{last_body}");

    // 其他 IP 不受影响(per-IP 隔离)。
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.8.8.8")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": "nobody", "password": "wrong-password" }))
                .unwrap(),
        ))
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_is_no_op_ok() {
    let app = make_app().await.unwrap();
    let req = auth_req(Method::POST, "/api/auth/logout", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}
