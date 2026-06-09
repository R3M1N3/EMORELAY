mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;

#[tokio::test]
async fn user_list_includes_rule_count_and_traffic() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    // 1) 建用户
    let req = auth_req(
        Method::POST,
        "/api/users",
        t,
        Some(json!({
            "username": "alice",
            "password": "alice12345",
            "role": "user",
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create user failed: {body}");
    let alice_id = body["id"].as_i64().unwrap();
    assert_eq!(body["rule_count"], 0, "create 路径默认 0");
    assert_eq!(body["total_traffic_bytes"], 0);

    // 2) 列表里 alice 也是 0
    let req = auth_req(Method::GET, "/api/users?page_size=50", t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let alice = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == alice_id)
        .expect("alice in list");
    assert_eq!(alice["rule_count"], 0);
    assert_eq!(alice["total_traffic_bytes"], 0);

    // 3) 建节点 + 直接 INSERT 一条规则归属 alice + 设流量 (跳过 /api/rules 端点是 admin/owner 路径,
    //    rule.user_id 会被覆盖成 admin。直接 SQL 注入 user_id 是 alice。)
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n1" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, \
                                    target_host, target_port, enabled, rx_bytes, tx_bytes) \
         VALUES (?, ?, 'r1', 'tcp', '0.0.0.0', 20001, '1.2.3.4', 80, 1, 100, 200)",
    )
    .bind(alice_id)
    .bind(node_id)
    .execute(&app.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, \
                                    target_host, target_port, enabled, rx_bytes, tx_bytes) \
         VALUES (?, ?, 'r2', 'udp', '0.0.0.0', 20002, '1.2.3.4', 80, 1, 50, 0)",
    )
    .bind(alice_id)
    .bind(node_id)
    .execute(&app.state.pool)
    .await
    .unwrap();

    // 4) 列表里 alice 应该 rule_count=2, total_traffic_bytes=350
    let req = auth_req(Method::GET, "/api/users?page_size=50", t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let alice = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == alice_id)
        .expect("alice in list");
    assert_eq!(alice["rule_count"], 2);
    assert_eq!(alice["total_traffic_bytes"], 350);
}

#[tokio::test]
async fn user_list_soft_deleted_rules_do_not_count() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    let req = auth_req(
        Method::POST,
        "/api/users",
        t,
        Some(json!({ "username": "bob", "password": "bob12345", "role": "user" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let bob_id = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "n1" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    // 一条软删 + 一条活跃,只算活跃。
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, \
                                    target_host, target_port, enabled, rx_bytes, tx_bytes, deleted_at) \
         VALUES (?, ?, 'gone', 'tcp', '0.0.0.0', 21001, '1.2.3.4', 80, 1, 999, 999, datetime('now'))",
    )
    .bind(bob_id)
    .bind(node_id)
    .execute(&app.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, \
                                    target_host, target_port, enabled, rx_bytes, tx_bytes) \
         VALUES (?, ?, 'live', 'tcp', '0.0.0.0', 21002, '1.2.3.4', 80, 1, 7, 3)",
    )
    .bind(bob_id)
    .bind(node_id)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let req = auth_req(Method::GET, "/api/users?page_size=50", t, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let bob = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == bob_id)
        .expect("bob in list");
    assert_eq!(bob["rule_count"], 1, "soft-deleted rule excluded");
    assert_eq!(bob["total_traffic_bytes"], 10);
}

#[tokio::test]
async fn user_with_zero_rules_returns_zero_aggregates() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    // admin 自己一开始没规则
    let req = auth_req(Method::GET, "/api/users?page_size=50", t, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let admin = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "admin")
        .expect("admin in list");
    assert_eq!(admin["rule_count"], 0);
    assert_eq!(admin["total_traffic_bytes"], 0);
}
