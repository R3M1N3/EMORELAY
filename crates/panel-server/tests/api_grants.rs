// P7: 节点/隧道使用授权(默认拒绝)集成测试。
// 语义:普通用户只能看到/使用被授权的节点与隧道;admin 不受限;
// 撤销授权不影响存量规则(保留运行),仅禁止新建。
mod common;

use axum::http::{Method, StatusCode};
use panel_server::models::tunnel::Tunnel;
use serde_json::json;

/// 建一个 online 节点,返回 id。
async fn seed_node(app: &common::TestApp, name: &str) -> i64 {
    sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES (?, 'x', 'online', '9.9.9.9', 20000, 20100)",
    )
    .bind(name)
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

// 默认拒绝:未授权用户节点列表为空、单条 404、建规则被拒。
#[tokio::test]
async fn ungranted_user_sees_nothing_and_cannot_create_rule() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app, "n-a").await;
    let (_uid, token) = common::make_user_token(&app, "u1", "password123").await.unwrap();

    let req = common::auth_req(Method::GET, "/api/nodes", &token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0, "未授权应看不到任何节点: {body}");

    let req = common::auth_req(Method::GET, &format!("/api/nodes/{node_id}"), &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND, "未授权节点按不存在处理");

    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": node_id, "name": "r", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("无权使用该节点"), "{body}");
}

// 授权后:列表可见(净化视图)、可建规则;admin 全程不受限。
#[tokio::test]
async fn granted_user_sees_node_and_creates_rule() {
    let app = common::make_app().await.unwrap();
    let node_a = seed_node(&app, "n-a").await;
    let _node_b = seed_node(&app, "n-b").await;
    let (uid, token) = common::make_user_token(&app, "u2", "password123").await.unwrap();
    common::grant_node(&app, uid, node_a).await;

    // 只见被授权的 n-a,不见 n-b
    let req = common::auth_req(Method::GET, "/api/nodes", &token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["items"][0]["id"], node_a);
    assert_eq!(body["items"][0]["grpc_endpoint"], "", "用户视图必须净化");

    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": node_a, "name": "ok", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    // admin 不需要授权
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({ "node_id": _node_b, "name": "adm", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
}

// 撤销授权:存量规则保留,新建被拒。
#[tokio::test]
async fn revoking_grant_keeps_existing_rule_but_blocks_new() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app, "n-r").await;
    let (uid, token) = common::make_user_token(&app, "u3", "password123").await.unwrap();
    common::grant_node(&app, uid, node_id).await;

    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": node_id, "name": "keep", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();

    // admin 全量替换为空 = 撤销
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/users/{uid}"),
        &app.admin_token,
        Some(json!({ "granted_node_ids": [] })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    // 存量规则仍可见可用
    let req = common::auth_req(Method::GET, &format!("/api/rules/{rule_id}"), &token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "撤销授权不应影响存量规则: {body}");

    // 新建被拒
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": node_id, "name": "new", "protocol": "tcp", "target_host": "1.2.3.4", "target_port": 443 })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// 隧道授权:未授权 list 空/get 404/建隧道规则拒;授权后全放开(入口节点随隧道授权,
// 不需单独节点授权)。
#[tokio::test]
async fn tunnel_grant_gates_visibility_and_rule_creation() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_node(&app, "t-entry").await;
    let n2 = seed_node(&app, "t-exit").await;
    let tid = Tunnel::create_with_hops(&app.state.pool, "tun1", "tcp", &[(0, n1, None), (1, n2, Some(30001))])
        .await
        .unwrap();
    let (uid, token) = common::make_user_token(&app, "u4", "password123").await.unwrap();

    let req = common::auth_req(Method::GET, "/api/tunnels", &token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "授权用户应可访问隧道列表: {body}");
    assert_eq!(body["total"], 0);

    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);

    let rule_body = json!({ "node_id": n1, "name": "tr", "protocol": "tcp",
        "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid, "listen_port": 20050 });
    let req = common::auth_req(Method::POST, "/api/rules", &token, Some(rule_body.clone())).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("无权使用该隧道"), "{body}");

    common::grant_tunnel(&app, uid, tid).await;

    let req = common::auth_req(Method::GET, "/api/tunnels", &token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["items"][0]["id"], tid);

    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 入口节点未单独授权,但隧道授权已覆盖
    let req = common::auth_req(Method::POST, "/api/rules", &token, Some(rule_body)).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    // 端口池豁免仅限 admin:普通用户的隧道规则监听端口必须在节点端口池内。
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &token,
        Some(json!({ "node_id": n1, "name": "oob", "protocol": "tcp",
            "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid, "listen_port": 9000 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("端口池"), "{body}");

    // admin 不受池约束(隧道 ingress 业务端口)
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({ "node_id": n1, "name": "adm-oob", "protocol": "tcp",
            "target_host": "1.2.3.4", "target_port": 443, "tunnel_id": tid, "listen_port": 9001 })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
}

// 授权管理端点:create/update 写入、users/{id}/grants 回显、节点/隧道反向列表、admin-only。
#[tokio::test]
async fn grants_endpoints_roundtrip_and_admin_only() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_node(&app, "g-1").await;
    let n2 = seed_node(&app, "g-2").await;
    let tid = Tunnel::create_with_hops(&app.state.pool, "g-tun", "tcp", &[(0, n1, None), (1, n2, Some(30002))])
        .await
        .unwrap();

    // create 带授权
    let req = common::auth_req(
        Method::POST,
        "/api/users",
        &app.admin_token,
        Some(json!({ "username": "grantee5", "password": "password123", "role": "user",
                     "granted_node_ids": [n1, n2], "granted_tunnel_ids": [tid] })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let uid = body["id"].as_i64().unwrap();

    // 回显
    let req = common::auth_req(Method::GET, &format!("/api/users/{uid}/grants"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["granted_node_ids"].as_array().unwrap().len(), 2);
    assert_eq!(body["granted_tunnel_ids"], json!([tid]));

    // update 全量替换:只留 n2
    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/users/{uid}"),
        &app.admin_token,
        Some(json!({ "granted_node_ids": [n2] })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let req = common::auth_req(Method::GET, &format!("/api/users/{uid}/grants"), &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["granted_node_ids"], json!([n2]), "全量替换语义");
    assert_eq!(body["granted_tunnel_ids"], json!([tid]), "未传的隧道授权保持不变");

    // 节点/隧道反向列表
    let req = common::auth_req(Method::GET, &format!("/api/nodes/{n2}/grants"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["username"], "grantee5", "{body}");
    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}/grants"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["username"], "grantee5", "{body}");

    // 普通用户访问授权端点一律 403
    let (_, token) = common::make_user_token(&app, "u6", "password123").await.unwrap();
    for path in [
        format!("/api/users/{uid}/grants"),
        format!("/api/nodes/{n2}/grants"),
        format!("/api/tunnels/{tid}/grants"),
    ] {
        let req = common::auth_req(Method::GET, &path, &token, None).unwrap();
        let (status, _) = common::send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "{path} 应 admin-only");
    }
}

// 无效/软删 id 静默跳过,不写入授权。
#[tokio::test]
async fn set_grants_skips_deleted_and_invalid_ids() {
    let app = common::make_app().await.unwrap();
    let node_id = seed_node(&app, "del-1").await;
    sqlx::query("UPDATE nodes SET deleted_at = datetime('now') WHERE id = ?")
        .bind(node_id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (uid, _) = common::make_user_token(&app, "u7", "password123").await.unwrap();

    let req = common::auth_req(
        Method::PATCH,
        &format!("/api/users/{uid}"),
        &app.admin_token,
        // 重复 id 去重容错,软删/不存在 id 静默跳过
        Some(json!({ "granted_node_ids": [node_id, node_id, 99999], "granted_tunnel_ids": [99999, 99999] })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    let req = common::auth_req(Method::GET, &format!("/api/users/{uid}/grants"), &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["granted_node_ids"], json!([]), "软删/无效节点应被跳过: {body}");
    assert_eq!(body["granted_tunnel_ids"], json!([]), "{body}");
}
