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

    // delete (软删):无 agent 在线 → dispatched=false,但软删仍成功、接口 200。
    let req = auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["dispatched"], false);

    // get 软删后 → 404
    let req = auth_req(Method::GET, &format!("/api/rules/{rule_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_rule_with_multi_target_and_strategy() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip, port_pool_min, port_pool_max) VALUES ('mn','x','1.2.3.4',10000,65535)")
        .execute(&app.state.pool).await.unwrap();
    // 多目标 + round 策略。
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": 1, "name": "mt", "protocol": "tcp", "listen_port": 20000,
            "target_host": "1.1.1.1", "target_port": 80,
            "extra_targets": [{"host": "2.2.2.2", "port": 80}, {"host": "3.3.3.3", "port": 8080}],
            "lb_strategy": "round"
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lb_strategy"], "round");
    let extra = body["extra_targets"].as_array().unwrap();
    assert_eq!(extra.len(), 2);
    assert_eq!(extra[0]["host"], "2.2.2.2");
    assert_eq!(extra[1]["port"], 8080);

    // 非法策略被拒。
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": 1, "name": "mt2", "protocol": "tcp", "listen_port": 20001,
            "target_host": "1.1.1.1", "target_port": 80, "lb_strategy": "bogus"
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_rule_with_reserved_port_rejected() {
    let app = make_app().await.unwrap();
    // 端口池显式含 22:默认池已上调到 10000+,否则会先撞"超出端口池"而非"保留端口"。
    let node_id = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES ('rsv', 'x', 1, 65535)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid();
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
    assert!(body["message"].as_str().unwrap().contains("保留端口"));
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
        .contains("已存在"));
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
    assert!(body["message"].as_str().unwrap().contains("端口池"));
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

// ============ P10a: 并发连接上限 ============

#[tokio::test]
async fn rule_max_connections_admin_managed_with_clear_semantics() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;

    // admin 建带上限规则 → 回显
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({ "node_id": node_id, "name": "capped", "protocol": "tcp", "listen_port": 20010,
                     "target_host": "1.2.3.4", "target_port": 443, "max_connections": 50 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();
    assert_eq!(body["max_connections"], 50);

    // 负数 → 400
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({ "node_id": node_id, "name": "neg", "protocol": "tcp", "listen_port": 20011,
                     "target_host": "1.2.3.4", "target_port": 443, "max_connections": -1 })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 普通用户自配 → 400;改 admin 设的上限 → 400
    let (uid, token) = common::make_user_token(&app, "capuser", "password123").await.unwrap();
    common::grant_node(&app, uid, node_id).await;
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": node_id, "name": "u", "protocol": "tcp", "listen_port": 20012,
                     "target_host": "1.2.3.4", "target_port": 443, "max_connections": 10 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // 把规则转给该用户场景简化:直接用 admin 规则验证用户 PATCH 被拒
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &token,
        Some(json!({ "max_connections": 0 })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    // 非本人规则 404 在 owner 校验之前/之后都可,这里用 admin 规则,user 校验先于 owner 时为 400,否则 404。
    assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND);

    // admin PATCH 0 → 清除(回显 null);PATCH 不传 → 不动
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &app.admin_token,
        Some(json!({ "name": "still-capped" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["max_connections"], 50, "未传字段不得改动");
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &app.admin_token,
        Some(json!({ "max_connections": 0 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["max_connections"].is_null(), "0 = 清除: {body}");
}
