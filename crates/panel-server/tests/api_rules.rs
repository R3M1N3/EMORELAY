mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send, TestApp};
use serde_json::json;

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

#[tokio::test]
async fn rule_full_cycle() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;

    // create
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "test-rule",
            "protocol": "tcp",
            "listen_port": 20000,
            "target_host": "127.0.0.1",
            "target_port": 8080,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let rule_id = body["id"].as_i64().unwrap();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["listen_port"], 20000);

    // disable
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/disable"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);

    // get → enabled false
    let req = auth_req(Method::GET, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["enabled"], false);

    // enable
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/enable"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);

    // restart(无 agent 在线,dispatched=false 但接口仍 200)
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/restart"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dispatched"], false);

    // delete (软删)
    let req = auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // get 软删后 → 404
    let req = auth_req(Method::GET, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_rule_with_reserved_port_rejected() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id,
            "name": "block-22",
            "protocol": "tcp",
            "listen_port": 22,
            "target_host": "1.2.3.4",
            "target_port": 80,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("reserved"));
}

#[tokio::test]
async fn duplicate_listen_binding_rejected() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;
    let payload = json!({
        "node_id": node_id,
        "name": "first",
        "protocol": "tcp",
        "listen_port": 25000,
        "target_host": "1.2.3.4",
        "target_port": 80,
    });
    let req = auth_req(Method::POST, "/api/rules", t, Some(payload.clone())).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let mut second = payload.clone();
    second["name"] = json!("second");
    let req = auth_req(Method::POST, "/api/rules", t, Some(second)).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
}

#[tokio::test]
async fn create_rule_outside_port_pool_rejected() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    // 创建一个端口池窄的节点
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "narrow", "port_pool_min": 30000, "port_pool_max": 31000 })),
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
            "name": "out-of-pool",
            "protocol": "tcp",
            "listen_port": 20000,  // 不在 30000-31000
            "target_host": "1.2.3.4",
            "target_port": 80,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("port pool"));
}

#[tokio::test]
async fn auto_stop_when_traffic_exceeds_limit() {
    use panel_server::grpc::service::auto_stop_if_exceeded;

    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;

    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "limited",
            "protocol": "tcp",
            "listen_port": 30000,
            "target_host": "1.2.3.4",
            "target_port": 80,
            "traffic_limit_bytes": 100,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let rule_id = body["id"].as_i64().unwrap();
    assert_eq!(body["enabled"], true);

    // 模拟流量已累计超 limit(由 report_rule_stats 的 UPDATE 完成,此处直接 UPDATE 模拟)
    sqlx::query("UPDATE forward_rules SET rx_bytes = 80, tx_bytes = 40 WHERE id = ?")
        .bind(rule_id)
        .execute(&app.state.pool)
        .await
        .unwrap();

    // 触发自动停判定
    auto_stop_if_exceeded(&app.state, rule_id).await.unwrap();

    // 验证 enabled=0
    let req = auth_req(Method::GET, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["enabled"], false);

    // 第二次调用 idempotent — 不报错也不改状态
    auto_stop_if_exceeded(&app.state, rule_id).await.unwrap();
}

#[tokio::test]
async fn auto_stop_when_expires_at_past() {
    use panel_server::grpc::service::auto_stop_if_exceeded;

    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;

    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "expired",
            "protocol": "tcp",
            "listen_port": 30001,
            "target_host": "1.2.3.4",
            "target_port": 80,
            "expires_at": "2020-01-01 00:00:00",
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let rule_id = body["id"].as_i64().unwrap();
    assert_eq!(body["enabled"], true);

    auto_stop_if_exceeded(&app.state, rule_id).await.unwrap();

    let req = auth_req(Method::GET, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["enabled"], false);
}
