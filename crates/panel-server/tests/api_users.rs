mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
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

#[tokio::test]
async fn user_quota_fields_roundtrip() {
    let app = common::make_app().await.unwrap();
    // create 带 expires_at + traffic_limit_bytes_30d
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "quotauser",
            "password": "password123",
            "role": "user",
            "expires_at": "2030-01-01T00:00",
            "traffic_limit_bytes_30d": 1073741824_i64
        })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let uid = body["id"].as_i64().unwrap();
    // normalize 后统一空格分隔格式
    assert_eq!(body["expires_at"], "2030-01-01 00:00:00");
    assert_eq!(body["traffic_limit_bytes_30d"], 1073741824_i64);
    assert_eq!(body["period_used_bytes_cached"], 0);
    assert_eq!(body["period_remaining_bytes"], 1073741824_i64);

    // PATCH 置空协议:expires_at="" 清除;limit=0 清除
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/users/{uid}"),
        &app.admin_token,
        Some(json!({ "expires_at": "", "traffic_limit_bytes_30d": 0 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["expires_at"].is_null());
    assert!(body["traffic_limit_bytes_30d"].is_null());
    assert!(body["period_remaining_bytes"].is_null());
}

#[tokio::test]
async fn user_create_rejects_bad_expires_format() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "badexp", "password": "password123", "role": "user",
            "expires_at": "not-a-date"
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn expired_user_cannot_login() {
    let app = common::make_app().await.unwrap();
    // 直接建一个已过期用户
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({
            "username": "expired1", "password": "password123", "role": "user",
            "expires_at": "2020-01-01 00:00:00"
        })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let login = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "127.0.0.1")
        .body(Body::from(
            serde_json::to_vec(&json!({ "username": "expired1", "password": "password123" }))
                .unwrap(),
        ))
        .unwrap();
    let (status, body) = common::send(app.app.clone(), login).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["message"], "account_expired");
}

// ============ P4: 服务端搜索 ============

#[tokio::test]
async fn users_list_server_side_search() {
    let app = make_app().await.unwrap();
    for name in ["alice", "bob"] {
        let req = auth_req(
            Method::POST,
            "/api/users",
            &app.admin_token,
            Some(json!({ "username": name, "password": "password123", "role": "user" })),
        )
        .unwrap();
        let (status, body) = send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let req = auth_req(Method::GET, "/api/users?search=ali", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["items"][0]["username"], "alice");

    // 通配符按字面量处理
    let req = auth_req(Method::GET, "/api/users?search=%25", &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 0, "{body}");
}
