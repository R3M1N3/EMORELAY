mod common;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

async fn seed_node_named(app: &common::TestApp, name: &str) -> i64 {
    let res = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, port_pool_min, port_pool_max) \
         VALUES (?, 'x', 20000, 29999)",
    )
    .bind(name)
    .execute(&app.state.pool)
    .await
    .unwrap();
    res.last_insert_rowid()
}

async fn create_rule(app: &common::TestApp, body: Value) -> Value {
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token, Some(body)).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn export_then_reimport_restores_rules() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node_named(&app, "io-node").await;
    // 带 profile 的规则
    let req = common::auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "io-100m", "bandwidth_mbps": 100 })),
    )
    .unwrap();
    let (_, p) = common::send(app.app.clone(), req).await.unwrap();
    let pid = p["id"].as_i64().unwrap();

    let r = create_rule(
        &app,
        json!({ "node_id": node_id, "name": "io-r1", "protocol": "tcp_udp", "listen_port": 20000,
                "target_host": "1.2.3.4", "target_port": 443, "bandwidth_profile_id": pid }),
    )
    .await;
    let rule_id = r["id"].as_i64().unwrap();

    // export
    let req = common::auth_req(Method::GET, "/api/rules/export", &app.admin_token, None).unwrap();
    let (status, exported) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{exported}");
    let items = exported.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["node_name"], "io-node");
    assert_eq!(items[0]["bandwidth_profile_name"], "io-100m");
    assert!(items[0]["tunnel_name"].is_null());
    assert!(items[0].get("id").is_none(), "导出不含 id");
    assert!(items[0].get("user_id").is_none(), "导出不含 user_id");

    // 删掉规则
    let req = common::auth_req(
        Method::DELETE,
        &format!("/api/rules/{rule_id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // dry-run 预览:action=create 不写库
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=1",
        &app.admin_token,
        Some(exported.clone()),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rules WHERE deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "dry_run 不得写库");

    // 实导
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=0",
        &app.admin_token,
        Some(exported),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM forward_rules WHERE deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "规则数恢复");
}

#[tokio::test]
async fn import_marks_missing_node_as_error_without_write() {
    let app = common::make_app().await.unwrap();
    let payload = json!([{
        "name": "ghost", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20001,
        "target_host": "1.2.3.4", "target_port": 443, "enabled": true,
        "node_name": "no-such-node", "tunnel_name": null, "bandwidth_profile_name": null
    }]);
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=1",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "error");
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("node not found"));
}

#[tokio::test]
async fn import_conflict_strategies_skip_and_overwrite() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node_named(&app, "io2").await;
    create_rule(
        &app,
        json!({ "node_id": node_id, "name": "exist", "protocol": "tcp", "listen_port": 20010,
                "target_host": "1.1.1.1", "target_port": 80 }),
    )
    .await;
    let payload = json!([{
        "name": "incoming", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20010,
        "target_host": "9.9.9.9", "target_port": 443, "enabled": true,
        "node_name": "io2", "tunnel_name": null, "bandwidth_profile_name": null
    }]);

    // skip
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=0",
        &app.admin_token,
        Some(payload.clone()),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "skip");
    let host: String = sqlx::query_scalar(
        "SELECT target_host FROM forward_rules WHERE listen_port = 20010 AND deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(host, "1.1.1.1", "skip 不得改动现有规则");

    // overwrite → PATCH 现有规则
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=overwrite&dry_run=0",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "overwrite", "{report}");
    let host: String = sqlx::query_scalar(
        "SELECT target_host FROM forward_rules WHERE listen_port = 20010 AND deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(host, "9.9.9.9");
}

#[tokio::test]
async fn import_rejects_tunnel_items_and_requires_admin() {
    let app = common::make_app().await.unwrap();
    seed_node_named(&app, "io3").await;
    let payload = json!([{
        "name": "tun", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20020,
        "target_host": "1.2.3.4", "target_port": 443, "enabled": true,
        "node_name": "io3", "tunnel_name": "hk-jp", "bandwidth_profile_name": null
    }]);
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?dry_run=1",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (_, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(report["items"][0]["action"], "error");
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("tunnel"));

    // 非 admin → 403
    let (_uid, token) = common::make_user_token(&app, "iouser", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/rules/export", &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
