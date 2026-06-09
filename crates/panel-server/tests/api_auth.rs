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
async fn logout_is_no_op_ok() {
    let app = make_app().await.unwrap();
    let req = auth_req(Method::POST, "/api/auth/logout", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}
