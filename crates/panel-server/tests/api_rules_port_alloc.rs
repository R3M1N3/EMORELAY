mod common;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

/// 建一个 port_pool [25000, 25005] 的节点,返回 node_id。
async fn seed_node(app: &common::TestApp) -> i64 {
    let res = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES ('palloc', 'x', 25000, 25005)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    res.last_insert_rowid()
}

async fn create_rule(app: &common::TestApp, body: Value) -> (StatusCode, Value) {
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token, Some(body)).unwrap();
    common::send(app.app.clone(), req).await.unwrap()
}

#[tokio::test]
async fn auto_alloc_picks_smallest_free_port() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "a1", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25000);

    // 第二条跳过已占用
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "a2", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25001);
}

#[tokio::test]
async fn auto_alloc_skips_reserved_ports() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    sqlx::query("UPDATE system_settings SET value = '[25000, 25001]' WHERE key = 'reserved_ports'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "r1", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25002);
}

#[tokio::test]
async fn reserved_ports_fail_closed_on_corrupt_value() {
    let app = common::make_app().await.unwrap();
    // 节点端口池覆盖 22,以隔离「保留端口」检查(否则 22 会先被端口池范围挡)。
    let res = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES ('failclosed', 'x', 22, 30000)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let node_id = res.last_insert_rowid();
    // 模拟配置损坏:把 reserved_ports 改成非法 JSON(直接改库,绕过 API 写时校验)。
    sqlx::query("UPDATE system_settings SET value = 'corrupt-not-json' WHERE key = 'reserved_ports'")
        .execute(&app.state.pool)
        .await
        .unwrap();
    // fail-closed:解析失败应回退默认保留集([22,80,443,3306,5432]),listen_port=22 仍被拒。
    // (旧实现解析失败返回空集 → 22 会被放行,本测试可区分。)
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "fc", "protocol": "tcp", "listen_port": 22, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "配置损坏时保留端口应 fail-closed 拒绝默认保留端口 22: {body}"
    );
}

#[tokio::test]
async fn auto_alloc_respects_protocol_mutex() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    // 占 25000:tcp_udp(与 tcp 和 udp 都互斥)
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m0", "protocol": "tcp_udp", "listen_port": 25000, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // 占 25001:udp
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m1", "protocol": "udp", "listen_port": 25001, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 自动分配 tcp:25000 被 tcp_udp 互斥;25001 的 udp 与 tcp 不互斥 → 拿 25001
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m2", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25001);

    // 自动分配 tcp_udp:25000(tcp_udp)/25001(udp+tcp) 都冲突 → 25002
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "m3", "protocol": "tcp_udp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listen_port"], 25002);
}

#[tokio::test]
async fn auto_alloc_pool_exhausted_returns_400() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    for p in 25000..=25005_i64 {
        let (status, _) = create_rule(
            &app,
            json!({ "node_id": node_id, "name": format!("f{p}"), "protocol": "tcp_udp", "listen_port": p, "target_host": "1.2.3.4", "target_port": 443 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "overflow", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("无可用端口"), "{body}");
}

#[tokio::test]
async fn explicit_listen_port_still_validated() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app).await;
    // 显式端口走旧校验路径:池外 → 400
    let (status, _) = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "e1", "protocol": "tcp", "listen_port": 30000, "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
