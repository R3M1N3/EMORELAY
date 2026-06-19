//! C2:规则级 send_proxy_protocol(admin 管控)落库/回显/权限。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, make_user_token, send, TestApp};
use serde_json::json;

async fn make_node(app: &TestApp) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "n1" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    body["node"]["id"].as_i64().unwrap()
}

#[tokio::test]
async fn admin_set_send_proxy_protocol_roundtrips_and_toggles() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;

    // 创建带 send_proxy_protocol=true 的规则。
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "pp", "protocol": "tcp",
            "listen_port": 34001, "target_host": "1.2.3.4", "target_port": 80,
            "send_proxy_protocol": true,
        })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["send_proxy_protocol"], true, "创建应回显 true: {body}");
    let rid = body["id"].as_i64().unwrap();

    // GET 详情回显 true。
    let req = auth_req(Method::GET, &format!("/api/rules/{rid}"), &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["send_proxy_protocol"], true);

    // PATCH 关闭 → false。
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rid}"),
        &app.admin_token,
        Some(json!({ "send_proxy_protocol": false })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["send_proxy_protocol"], false, "PATCH 应关闭: {body}");
}

async fn seed_online_node(app: &TestApp, name: &str) -> i64 {
    let req = auth_req(Method::POST, "/api/nodes", &app.admin_token, Some(json!({ "name": name }))).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let id = body["node"]["id"].as_i64().unwrap();
    sqlx::query("UPDATE nodes SET status = 'online', public_ip = '198.51.100.' || id WHERE id = ?")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn tunnel_rule_rejects_send_proxy_protocol() {
    // PROXY 仅非隧道 TCP 生效;隧道规则开启会被 split 静默丢弃,入口应直接 400(I2 修复)。
    let app = make_app().await.unwrap();
    let entry = seed_online_node(&app, "entry").await;
    let exit = seed_online_node(&app, "exit").await;
    let req = auth_req(
        Method::POST,
        "/api/tunnels",
        &app.admin_token,
        Some(json!({ "name": "t1", "transport": "tcp", "node_ids": [entry, exit] })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "create tunnel: {body}");
    let tid = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": entry, "name": "tp", "protocol": "tcp",
            "target_host": "1.2.3.4", "target_port": 80,
            "tunnel_id": tid, "send_proxy_protocol": true,
        })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "隧道规则不应允许 PROXY protocol: {body}");
}

#[tokio::test]
async fn non_admin_cannot_set_send_proxy_protocol() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;

    // 普通用户尝试开启 → 400(admin 管控字段)。
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &alice_token,
        Some(json!({
            "node_id": node_id, "name": "pp", "protocol": "tcp",
            "listen_port": 34010, "target_host": "1.2.3.4", "target_port": 80,
            "send_proxy_protocol": true,
        })),
    )
    .unwrap();
    let (s, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "普通用户不得配置 PROXY protocol");
}
