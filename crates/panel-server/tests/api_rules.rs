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

// P2: 规则级到期/流量/带宽三个限制字段已下线,对应 auto_stop 测试随之删除;
// 用户级到期/配额覆盖在 user_quota sweeper 测试(Task 5)。
#[tokio::test]
async fn rule_with_profile_roundtrip_and_detach() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;

    // 建 profile(77 Mbps)
    let req = auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        t,
        Some(json!({ "name": "p77", "bandwidth_mbps": 77 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let profile_id = body["id"].as_i64().unwrap();

    // 带 profile 建规则 → bandwidth_mbps 回显 77
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "with-profile",
            "protocol": "tcp",
            "listen_port": 21000,
            "target_host": "1.2.3.4",
            "target_port": 80,
            "bandwidth_profile_id": profile_id,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();
    assert_eq!(body["bandwidth_profile_id"], profile_id);
    assert_eq!(body["bandwidth_mbps"], 77);

    // PATCH bandwidth_profile_id=0 → 解除关联,两字段回 null
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        t,
        Some(json!({ "bandwidth_profile_id": 0 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["bandwidth_profile_id"].is_null());
    assert!(body["bandwidth_mbps"].is_null());

    // PATCH 不存在的 profile → 400
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        t,
        Some(json!({ "bandwidth_profile_id": 99999 })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_rule_without_profile_returns_null_bandwidth() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let node_id = make_node(&app).await;

    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "no-profile",
            "protocol": "tcp",
            "listen_port": 30000,
            "target_host": "1.2.3.4",
            "target_port": 80,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    assert_eq!(body["enabled"], true);
    assert!(body["bandwidth_profile_id"].is_null());
    assert!(body["bandwidth_mbps"].is_null());
}
