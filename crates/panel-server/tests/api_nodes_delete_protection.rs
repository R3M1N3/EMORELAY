mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;

#[tokio::test]
async fn delete_node_with_active_rules_returns_400() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    // 1. 创建节点
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-with-rule" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let node_id = body["node"]["id"].as_i64().unwrap();

    // 2. 在节点上创建规则
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "r1",
            "protocol": "tcp",
            "listen_port": 20000,
            "target_host": "1.2.3.4",
            "target_port": 443,
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 3. 删节点 → 400 + 错误消息含规则信息
    let req = auth_req(
        Method::DELETE,
        &format!("/api/nodes/{node_id}"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let msg = body["message"].as_str().unwrap();
    // 消息含「规则」或英文 "rule" + 规则名 "r1"
    assert!(
        msg.contains("规则") || msg.to_lowercase().contains("rule"),
        "message should mention rule(s): {msg}"
    );
    assert!(msg.contains("r1"), "message should contain rule name: {msg}");
}

#[tokio::test]
async fn delete_node_without_rules_succeeds() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-empty" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let req = auth_req(
        Method::DELETE,
        &format!("/api/nodes/{node_id}"),
        t,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn delete_node_after_rule_soft_deleted_succeeds() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n-revivable" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "r-soft",
            "protocol": "tcp",
            "listen_port": 21000,
            "target_host": "1.2.3.4",
            "target_port": 443,
        })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    // 软删规则
    let req = auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    send(app.app.clone(), req).await.unwrap();

    // 再删节点 → OK
    let req = auth_req(Method::DELETE, &format!("/api/nodes/{node_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}
