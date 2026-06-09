mod common;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use common::make_app;
use tower::ServiceExt;

#[tokio::test]
async fn install_sh_returns_bash_script_with_node_id() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh?node=42")
        .header("x-forwarded-for", "127.0.0.1")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/x-shellscript") || ct.starts_with("text/plain"),
        "unexpected content-type: {ct}"
    );
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(
        body.starts_with("#!/usr/bin/env bash") || body.starts_with("#!/bin/bash"),
        "expected bash shebang, got: {}", &body[..30.min(body.len())]
    );
    assert!(body.contains("AGENT_NODE_ID=42"), "missing AGENT_NODE_ID");
    assert!(body.contains("--token="), "missing --token= handling");
}

#[tokio::test]
async fn install_sh_missing_node_returns_400() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh")
        .header("x-forwarded-for", "127.0.0.1")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn install_sh_uses_endpoint_from_settings() {
    let app = make_app().await.unwrap();
    // 先设端点
    let req = common::auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({
            "settings": { "agent_control_endpoint": "https://relay.example.com:50051" }
        })),
    )
    .unwrap();
    common::send(app.app.clone(), req).await.unwrap();

    // 拉 install.sh
    let req = Request::builder()
        .method(Method::GET)
        .uri("/install.sh?node=7")
        .header("x-forwarded-for", "127.0.0.1")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert!(
        body.contains("AGENT_CONTROL_ENDPOINT=https://relay.example.com:50051"),
        "missing endpoint in env block"
    );
}

#[tokio::test]
async fn dist_unknown_arch_returns_404() {
    let app = make_app().await.unwrap();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/dist/node-agent-linux-mips")
        .header("x-forwarded-for", "127.0.0.1")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn install_sh_rate_limited_after_burst() {
    let app = make_app().await.unwrap();
    let mut ok_count = 0;
    let mut rate_limited = false;
    // 注入 x-forwarded-for 让 SmartIpKeyExtractor 能提取到 IP。
    // 70 次请求中前 60 次 burst 内应 OK，后续触发 429。
    for _ in 0..70 {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/install.sh?node=1")
            .header("x-forwarded-for", "127.0.0.1")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.app.clone().oneshot(req).await.unwrap();
        match resp.status() {
            StatusCode::OK => ok_count += 1,
            StatusCode::TOO_MANY_REQUESTS => {
                rate_limited = true;
                break;
            }
            other => panic!("unexpected status: {other}"),
        }
    }
    assert!(ok_count >= 1, "expected at least 1 OK before 429");
    assert!(
        rate_limited,
        "expected 429 within 70 attempts; ok_count={ok_count}"
    );
}
