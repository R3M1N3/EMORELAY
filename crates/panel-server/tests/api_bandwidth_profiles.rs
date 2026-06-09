mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn bandwidth_profile_crud_roundtrip() {
    let app = common::make_app().await.unwrap();
    // create
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "100m-shared", "bandwidth_mbps": 100, "description": "公用 100M" })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["id"].as_i64().unwrap();
    assert_eq!(body["bandwidth_mbps"], 100);

    // list
    let req = common::auth_req(Method::GET, "/api/bandwidth-profiles", &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["name"], "100m-shared");

    // patch
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        Some(json!({ "bandwidth_mbps": 50 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["bandwidth_mbps"], 50);

    // delete(无引用)
    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // get 已删 → 404
    let req = common::auth_req(
        Method::GET,
        &format!("/api/bandwidth-profiles/{id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bandwidth_profile_rejects_dup_name_and_bad_mbps() {
    let app = common::make_app().await.unwrap();
    // 第一次创建必须成功;第二次同名必须 400 —— 两个显式断言,唯一名保护回归时测试必红。
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "dup", "bandwidth_mbps": 10 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "dup", "bandwidth_mbps": 10 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "zero", "bandwidth_mbps": 0 })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn bandwidth_profile_delete_blocked_by_rule_reference() {
    let app = common::make_app().await.unwrap();
    // 建 profile
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "ref", "bandwidth_mbps": 30 })),
    )
    .unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let pid = body["id"].as_i64().unwrap();

    // 直接在 DB 建 node + 引用规则(避开 rules API 依赖)
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('n1', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port, bandwidth_profile_id) \
         VALUES (?, 1, 'r1', 'tcp', '0.0.0.0', 20001, '1.2.3.4', 443, ?)",
    )
    .bind(app.admin_user_id)
    .bind(pid)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/bandwidth-profiles/{pid}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("1"), "应包含引用规则数: {body}");
}

#[tokio::test]
async fn bandwidth_profiles_require_admin() {
    let app = common::make_app().await.unwrap();
    let (_uid, token) = common::make_user_token(&app, "normal1", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/bandwidth-profiles", &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
