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
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("节点不存在"));
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
    assert!(report["items"][0]["reason"].as_str().unwrap().contains("隧道"));

    // 非 admin → 403
    let (_uid, token) = common::make_user_token(&app, "iouser", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/rules/export", &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// P9: 导出按 tunnel_id 过滤,且隧道规则导出真实 tunnel_name。
#[tokio::test]
async fn export_filters_by_tunnel_id_with_real_tunnel_name() {
    use panel_server::models::tunnel::Tunnel;
    let app = common::make_app().await.unwrap();
    let n1 = seed_node_named(&app, "tio-entry").await;
    let n2 = seed_node_named(&app, "tio-exit").await;
    let tid = Tunnel::create_with_hops(&app.state.pool, "tio-tun", "tcp", &[(0, n1, None), (1, n2, Some(30001))])
        .await
        .unwrap();
    // 一条隧道规则 + 一条普通规则
    create_rule(
        &app,
        json!({ "node_id": n1, "name": "tio-tr", "protocol": "tcp", "listen_port": 20030,
                "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid }),
    )
    .await;
    create_rule(
        &app,
        json!({ "node_id": n1, "name": "tio-plain", "protocol": "tcp", "listen_port": 20031,
                "target_host": "1.2.3.4", "target_port": 443 }),
    )
    .await;

    let req = common::auth_req(
        Method::GET,
        &format!("/api/rules/export?tunnel_id={tid}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, exported) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{exported}");
    let items = exported.as_array().unwrap();
    assert_eq!(items.len(), 1, "只导出该隧道的规则: {exported}");
    assert_eq!(items[0]["name"], "tio-tr");
    assert_eq!(items[0]["tunnel_name"], "tio-tun", "隧道规则导出真实关联名");
}

// P9: target_node_id 给定时全部映射到该节点,忽略文件内 node_name;无效目标整体 400。
#[tokio::test]
async fn import_target_node_id_maps_all_items() {
    let app = common::make_app().await.unwrap();
    seed_node_named(&app, "src-node").await;
    let dst = seed_node_named(&app, "dst-node").await;
    // 两项在原实例分属不同节点(同端口),汇入单节点后第二项是文件内重复绑定 → error。
    let payload = json!([
        {
            "name": "moved", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20040,
            "target_host": "1.2.3.4", "target_port": 443, "enabled": true,
            "node_name": "src-node", "tunnel_name": null, "bandwidth_profile_name": null
        },
        {
            "name": "clash", "protocol": "tcp", "listen_ip": "0.0.0.0", "listen_port": 20040,
            "target_host": "5.6.7.8", "target_port": 443, "enabled": true,
            "node_name": "other-node", "tunnel_name": null, "bandwidth_profile_name": null
        }
    ]);

    // dry-run:不写库,文件内重复由 seen 检测报 error(否则预览两项都显示 create,失真)。
    let req = common::auth_req(
        Method::POST,
        &format!("/api/rules/import?strategy=overwrite&dry_run=1&target_node_id={dst}"),
        &app.admin_token,
        Some(payload.clone()),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create", "{report}");
    assert_eq!(report["items"][1]["action"], "error", "文件内重复绑定预览应报 error: {report}");
    assert!(report["items"][1]["reason"].as_str().unwrap().contains("重复"), "{report}");

    // 实导(overwrite 策略):第一项落库后,第二项经 DB 命中本会变 Overwrite 误覆盖第一项,
    // seen 检测把它拦成 error。
    let req = common::auth_req(
        Method::POST,
        &format!("/api/rules/import?strategy=overwrite&dry_run=0&target_node_id={dst}"),
        &app.admin_token,
        Some(payload.clone()),
    )
    .unwrap();
    let (status, report) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["items"][0]["action"], "create", "{report}");
    assert_eq!(report["items"][1]["action"], "error", "实导第二项不得覆盖第一项: {report}");
    let host: String = sqlx::query_scalar(
        "SELECT target_host FROM forward_rules WHERE name = 'moved' AND deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(host, "1.2.3.4", "第一项内容不得被第二项覆盖");
    let node_id: i64 = sqlx::query_scalar(
        "SELECT node_id FROM forward_rules WHERE name = 'moved' AND deleted_at IS NULL",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(node_id, dst, "规则应落在指定目标节点而非文件内 node_name");

    // 目标节点不存在 → 整体 400
    let req = common::auth_req(
        Method::POST,
        "/api/rules/import?strategy=skip&dry_run=1&target_node_id=99999",
        &app.admin_token,
        Some(payload),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
