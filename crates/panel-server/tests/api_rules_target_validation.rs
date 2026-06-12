mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

async fn seed_node(app: &common::TestApp) -> i64 {
    sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES ('tval', 'x', 25000, 25005)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

// 用户报的 bug:1.2.3 既非合法 IP 也非合法域名(TLD 纯数字),必须拒绝——与角色无关。
#[tokio::test]
async fn rejects_ip_shaped_garbage_target() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    for bad in ["1.2.3", "12345", "1.2.3.4.5"] {
        let req = common::auth_req(
            Method::POST,
            "/api/rules",
            &app.admin_token,
            Some(json!({ "node_id": node_id, "name": "g", "protocol": "tcp", "target_host": bad, "target_port": 443 })),
        )
        .unwrap();
        let (status, body) = common::send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} 应被拒: {body}");
    }
}

// 普通用户不得把目标指向回环/内网(防借节点中转访问 Agent 机内网服务)。
#[tokio::test]
async fn user_cannot_target_internal_ip() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let (uid, user_token) = common::make_user_token(&app, "bob", "bob1234567").await.unwrap();
    // P7: 授权节点,确保 400 来自内网目标校验而非授权拒绝。
    common::grant_node(&app, uid, node_id).await;
    for internal in ["127.0.0.1", "10.0.0.1", "192.168.1.1"] {
        let req = common::auth_req(
            Method::POST,
            "/api/rules",
            &user_token,
            Some(json!({ "node_id": node_id, "name": "i", "protocol": "tcp", "target_host": internal, "target_port": 443 })),
        )
        .unwrap();
        let (status, body) = common::send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{internal} 应被拒: {body}");
    }
}

// admin 不受内网限制(运维自有用途)。
#[tokio::test]
async fn admin_may_target_internal_ip() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({ "node_id": node_id, "name": "adm", "protocol": "tcp", "target_host": "10.0.0.1", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
}

// 公网域名目标对普通用户放行(域名指向内网拦不住,属已知局限)。
#[tokio::test]
async fn user_may_target_public_domain() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let (uid, user_token) = common::make_user_token(&app, "carol", "carol123456").await.unwrap();
    common::grant_node(&app, uid, node_id).await;
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &user_token,
        Some(json!({ "node_id": node_id, "name": "dom", "protocol": "tcp", "target_host": "example.com", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
}

// update 路径与 create 对称:普通用户不得把已有规则的目标改成内网。
#[tokio::test]
async fn user_update_cannot_switch_to_internal_ip() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let (uid, user_token) = common::make_user_token(&app, "dave", "dave1234567").await.unwrap();
    common::grant_node(&app, uid, node_id).await;
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &user_token,
        Some(json!({ "node_id": node_id, "name": "u", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().expect("rule id");

    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &user_token,
        Some(json!({ "target_host": "127.0.0.1" })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
