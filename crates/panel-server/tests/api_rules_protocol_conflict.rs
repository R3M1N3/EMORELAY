mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send, TestApp};
use serde_json::{json, Value};

async fn make_node(app: &TestApp) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "n1" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    body["node"]["id"].as_i64().unwrap()
}

async fn create_rule(
    app: &TestApp,
    node_id: i64,
    name: &str,
    protocol: &str,
    port: u16,
) -> (StatusCode, Value) {
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id,
            "name": name,
            "protocol": protocol,
            "listen_port": port,
            "target_host": "1.2.3.4",
            "target_port": 8080,
        })),
    )
    .unwrap();
    send(app.app.clone(), req).await.unwrap()
}

#[tokio::test]
async fn tcp_then_tcp_udp_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp", 20000).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = create_rule(&app, node_id, "b", "tcp_udp", 20000).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn udp_then_tcp_udp_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "udp", 20001).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = create_rule(&app, node_id, "b", "tcp_udp", 20001).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn tcp_udp_then_tcp_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp_udp", 20002).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = create_rule(&app, node_id, "b", "tcp", 20002).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn tcp_udp_then_udp_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp_udp", 20003).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = create_rule(&app, node_id, "b", "udp", 20003).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn tcp_and_udp_on_same_port_allowed() {
    // tcp 与 udp 在同一端口不冲突(实际 Agent 能同时 bind tcp socket 和 udp socket)。
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp", 20004).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, _) = create_rule(&app, node_id, "b", "udp", 20004).await;
    assert_eq!(s2, StatusCode::OK);
}

#[tokio::test]
async fn different_nodes_do_not_conflict() {
    let app = make_app().await.unwrap();
    let n1 = make_node(&app).await;
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "n2" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let n2 = body["node"]["id"].as_i64().unwrap();

    let (s1, _) = create_rule(&app, n1, "a", "tcp_udp", 20005).await;
    assert_eq!(s1, StatusCode::OK);
    // 另一节点同端口 tcp 应该被允许
    let (s2, _) = create_rule(&app, n2, "b", "tcp", 20005).await;
    assert_eq!(s2, StatusCode::OK);
}

#[tokio::test]
async fn update_listen_port_into_conflict_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp_udp", 20006).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, body) = create_rule(&app, node_id, "b", "tcp", 20007).await;
    assert_eq!(s2, StatusCode::OK);
    let b_id = body["id"].as_i64().unwrap();

    // 把 tcp 规则的端口改到 tcp_udp 占用的端口
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{b_id}"),
        &app.admin_token,
        Some(json!({ "listen_port": 20006 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn update_no_port_change_does_not_falsely_conflict() {
    // 改 target_host 但不改 listen_port 时,自身规则不应被自己当成冲突。
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, body) = create_rule(&app, node_id, "a", "tcp_udp", 20008).await;
    assert_eq!(s1, StatusCode::OK);
    let id = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{id}"),
        &app.admin_token,
        Some(json!({ "target_host": "9.9.9.9" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn update_listen_ip_into_conflict_rejected() {
    // A 在 0.0.0.0:P tcp_udp, B 在 127.0.0.1:P tcp; PATCH B.listen_ip=0.0.0.0 应被拦。
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, _) = create_rule(&app, node_id, "a", "tcp_udp", 20010).await;
    assert_eq!(s1, StatusCode::OK);
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id,
            "name": "b",
            "protocol": "tcp",
            "listen_ip": "127.0.0.1",
            "listen_port": 20010,
            "target_host": "1.2.3.4",
            "target_port": 8080,
        })),
    )
    .unwrap();
    let (s2, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s2, StatusCode::OK, "create B should pass: {body}");
    let b_id = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{b_id}"),
        &app.admin_token,
        Some(json!({ "listen_ip": "0.0.0.0" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("冲突"));
}

#[tokio::test]
async fn soft_deleted_rule_does_not_block_new_creation() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (s1, body) = create_rule(&app, node_id, "a", "tcp_udp", 20009).await;
    assert_eq!(s1, StatusCode::OK);
    let id = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::DELETE,
        &format!("/api/rules/{id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 软删后,同端口创建 tcp 不应被拦
    let (s2, _) = create_rule(&app, node_id, "b", "tcp", 20009).await;
    assert_eq!(s2, StatusCode::OK);
}
