mod common;

use axum::http::Method;
use emorelay_common::control::v1::{command::Body, TunnelRole};
use serde_json::json;

/// 建 N 个 online 节点(带 public_ip + port_pool),返回 ids。
async fn seed_online_nodes(app: &common::TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
             VALUES (?, 'x', 'online', ?, 30000, 30010)",
        )
        .bind(format!("dn{i}"))
        .bind(format!("10.1.0.{i}"))
        .execute(&app.state.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        ids.push(id);
    }
    ids
}

async fn create_tunnel(app: &common::TestApp, transport: &str, nodes: &[i64]) -> i64 {
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": format!("t-{transport}-{}", nodes.len()), "transport": transport, "node_ids": nodes }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    body["id"].as_i64().unwrap()
}

#[tokio::test]
async fn create_rule_on_tunnel_dispatches_per_hop_split_rules() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 3).await;
    // 模拟三个 Agent 在线。
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let tid = create_tunnel(&app, "tcp", &nodes).await;

    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();

    // 每个 hop 节点都收到一条带 tunnel 上下文的 ApplyRule。
    let expected_roles = [TunnelRole::Entry, TunnelRole::Mid, TunnelRole::Exit];
    for (i, rx) in rxs.iter_mut().enumerate() {
        let cmd = rx.try_recv().expect("hop should receive ApplyRule");
        let Some(Body::ApplyRule(apply)) = cmd.body else { panic!("expected ApplyRule") };
        let rule = apply.rule.expect("rule");
        assert_eq!(rule.id, rule_id);
        let t = rule.tunnel.expect("tunnel context");
        assert_eq!(t.role, expected_roles[i] as i32);
        assert_eq!(t.self_ordinal, i as u32);
        if i == 0 {
            assert_eq!(rule.listen_port, 20000);
            assert_eq!(t.next_hop_addr, "10.1.0.1");
        }
        if i > 0 {
            assert!(t.self_inter_port >= 30000, "mid/exit 监听 inter_port");
        }
    }
}

#[tokio::test]
async fn tls_tunnel_create_dispatches_credentials_to_each_hop() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let _tid = create_tunnel(&app, "tls", &nodes).await;

    for (i, rx) in rxs.iter_mut().enumerate() {
        let cmd = rx.try_recv().expect("hop should receive TunnelCredentials");
        let Some(Body::TunnelCredentials(c)) = cmd.body else { panic!("expected TunnelCredentials") };
        assert_eq!(c.ordinal, i as i32);
        assert!(c.server_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(c.client_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(c.ca_pem.contains("BEGIN CERTIFICATE"), "凭据必须自包含 CA");
    }
}

#[tokio::test]
async fn delete_rule_and_tunnel_dispatch_remove_and_revoke() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    // 规则/隧道建好后再上线订阅(只关心 delete 阶段的命令)。
    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();

    let req = common::auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), &app.admin_token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    for rx in rxs.iter_mut() {
        let cmd = rx.try_recv().expect("hop should receive RemoveRule");
        let Some(Body::RemoveRule(r)) = cmd.body else { panic!("expected RemoveRule") };
        assert_eq!(r.rule_id, rule_id);
    }

    let req = common::auth_req(Method::DELETE, &format!("/api/tunnels/{tid}"), &app.admin_token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    for rx in rxs.iter_mut() {
        let cmd = rx.try_recv().expect("hop should receive RevokeTunnelCredentials");
        let Some(Body::RevokeTunnelCredentials(r)) = cmd.body else { panic!("expected Revoke") };
        assert_eq!(r.tunnel_id, tid);
    }
}

#[tokio::test]
async fn reconcile_replays_tunnel_hop_rules_with_credentials_first() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, _body) = common::send(app.app.clone(), req).await.unwrap();
    // 非隧道规则也挂在 exit 节点上,确认一并 reconcile。
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
         VALUES (?, ?, 'plain', 'tcp', '0.0.0.0', 30005, '1.1.1.1', 80)",
    ).bind(app.admin_user_id).bind(nodes[1])
    .execute(&app.state.pool).await.unwrap();

    // exit 节点(mid 链路上无规则行)reconcile:凭据先行,再是本 hop 拆分 Rule + 非隧道规则。
    let cmds = panel_server::grpc::tunnel_dispatch::reconcile_commands_for_node(&app.state, nodes[1])
        .await
        .expect("reconcile");
    let mut saw_creds_at = None;
    let mut saw_hop_rule_at = None;
    let mut saw_plain_at = None;
    for (i, cmd) in cmds.iter().enumerate() {
        match &cmd.body {
            Some(Body::TunnelCredentials(c)) if c.tunnel_id == tid => saw_creds_at = Some(i),
            Some(Body::ApplyRule(a)) => {
                let r = a.rule.as_ref().unwrap();
                if let Some(t) = &r.tunnel {
                    assert_eq!(t.role, TunnelRole::Exit as i32, "exit 节点只该拿 exit 份");
                    saw_hop_rule_at = Some(i);
                } else {
                    saw_plain_at = Some(i);
                }
            }
            _ => {}
        }
    }
    let creds = saw_creds_at.expect("reconcile 必须含隧道凭据");
    let hop = saw_hop_rule_at.expect("reconcile 必须含本 hop 拆分 Rule");
    assert!(creds < hop, "凭据必须先于隧道规则下发");
    saw_plain_at.expect("非隧道规则也要 reconcile");
}

#[tokio::test]
async fn restart_tunnel_redispatches_credentials_and_restarts_rules() {
    let app = common::make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    let tid = create_tunnel(&app, "tls", &nodes).await;
    let req = common::auth_req(Method::POST, "/api/rules", &app.admin_token,
        Some(json!({ "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                     "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let rule_id = body["id"].as_i64().unwrap();

    let mut rxs: Vec<_> = nodes.iter().map(|n| app.state.dispatcher.subscribe(*n).0).collect();
    let req = common::auth_req(Method::POST, &format!("/api/tunnels/{tid}/restart"), &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["dispatched"], true);

    for rx in rxs.iter_mut() {
        // 凭据 + 该规则的 restart,顺序:凭据先。
        let c1 = rx.try_recv().expect("credentials");
        assert!(matches!(c1.body, Some(Body::TunnelCredentials(_))));
        let c2 = rx.try_recv().expect("restart");
        let Some(Body::RestartRule(r)) = c2.body else { panic!("expected RestartRule") };
        assert_eq!(r.rule_id, rule_id);
    }
}

#[tokio::test]
async fn create_tunnel_rejects_dialed_hop_without_public_ip() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_online_nodes(&app, 1).await[0];
    let bare = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES ('noip', 'x', 'online', '', 30000, 30010)",
    ).execute(&app.state.pool).await.unwrap().last_insert_rowid();
    let req = common::auth_req(Method::POST, "/api/tunnels", &app.admin_token,
        Some(json!({ "name": "noip-t", "transport": "tcp", "node_ids": [n1, bare] }))).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("公网 IP"));
}
