mod common;

use panel_server::models::tunnel::{Tunnel, TunnelHop};

#[tokio::test]
async fn create_tunnel_with_hops_and_read_back() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('hk', 'x', '1.1.1.1')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash, public_ip) VALUES ('jp', 'x', '2.2.2.2')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();

    let tid = Tunnel::create_with_hops(
        &app.state.pool, "hk-jp", "tcp",
        &[(0, n1, None), (1, n2, Some(30001))],
    ).await.unwrap();

    let t = Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().unwrap();
    assert_eq!(t.name, "hk-jp");
    assert_eq!(t.transport, "tcp");
    assert_eq!(t.status, "unknown");

    let hops = TunnelHop::list_for_tunnel(&app.state.pool, tid).await.unwrap();
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].ordinal, 0);
    assert_eq!(hops[0].node_id, n1);
    assert!(hops[0].inter_port.is_none());
    assert_eq!(hops[1].ordinal, 1);
    assert_eq!(hops[1].inter_port, Some(30001));
}

#[tokio::test]
async fn soft_delete_hides_tunnel_and_active_refs_counts() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let tid = Tunnel::create_with_hops(&app.state.pool, "t1", "tls",
        &[(0, n1, None), (1, n2, Some(30002))]).await.unwrap();

    assert_eq!(Tunnel::active_rule_refs(&app.state.pool, tid).await.unwrap(), 0);
    assert_eq!(Tunnel::soft_delete(&app.state.pool, tid).await.unwrap(), 1);
    assert!(Tunnel::find_by_id(&app.state.pool, tid).await.unwrap().is_none());
}

#[tokio::test]
async fn hops_using_node_detects_node_membership() {
    let app = common::make_app().await.unwrap();
    let n1 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('a','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let n2 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('b','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    Tunnel::create_with_hops(&app.state.pool, "t2", "tcp",
        &[(0, n1, None), (1, n2, Some(30003))]).await.unwrap();
    assert!(TunnelHop::node_in_active_tunnel(&app.state.pool, n2).await.unwrap());
    let n3 = sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('c','x')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    assert!(!TunnelHop::node_in_active_tunnel(&app.state.pool, n3).await.unwrap());
}

use axum::http::{Method, StatusCode};
use serde_json::json;

/// 建 N 个 online 节点,port_pool [30000,30010],返回 ids。
async fn seed_online_nodes(app: &common::TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
             VALUES (?, 'x', 'online', ?, 30000, 30010)",
        )
        .bind(format!("tn{i}")).bind(format!("10.0.0.{i}"))
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn create_tunnel_allocates_inter_ports_and_lists() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 3).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "hk-jp-us", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let tid = body["id"].as_i64().unwrap();

    let req = common::auth_req(Method::GET, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let hops = body["hops"].as_array().unwrap();
    assert_eq!(hops.len(), 3);
    assert!(hops[0]["inter_port"].is_null());
    let p1 = hops[1]["inter_port"].as_i64().unwrap();
    let p2 = hops[2]["inter_port"].as_i64().unwrap();
    assert!((30000..=30010).contains(&p1) && (30000..=30010).contains(&p2));

    let req = common::auth_req(Method::GET, "/api/tunnels", &app.admin_token, None).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["hops_count"], 3);
}

#[tokio::test]
async fn create_tunnel_rejects_short_chain_dup_and_offline() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0]] }))).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0], nodes[0]] }))).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let off = sqlx::query("INSERT INTO nodes (name, agent_token_hash, status) VALUES ('off','x','offline')")
        .execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "x", "transport": "tcp", "node_ids": [nodes[0], off] }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("online"));
}

#[tokio::test]
async fn delete_tunnel_blocked_by_rule_reference() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port, tunnel_id) \
         VALUES (?, ?, 'r', 'tcp', '0.0.0.0', 20000, '1.2.3.4', 443, ?)",
    ).bind(app.admin_user_id).bind(nodes[0]).bind(tid)
    .execute(&app.state.pool).await.unwrap();

    let req = common::auth_req(Method::DELETE, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("1"));
}

#[tokio::test]
async fn patch_only_name_and_requires_admin() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "t", "transport": "tcp", "node_ids": nodes }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let tid = body["id"].as_i64().unwrap();
    let req = common::auth_req(Method::PATCH, &format!("/api/tunnels/{tid}"), &app.admin_token,
        Some(json!({ "name": "renamed" }))).unwrap();
    let (s, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "renamed");
    let (_uid, token) = common::make_user_token(&app, "u", "password123").await.unwrap();
    let req = common::auth_req(Method::GET, "/api/tunnels", &token, None).unwrap();
    let (s, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::FORBIDDEN);
}
