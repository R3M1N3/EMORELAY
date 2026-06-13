mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send};
use emorelay_common::control::v1::{command::Body, ProbeResult};
use serde_json::json;

async fn make_node_and_rule(app: &common::TestApp) -> (i64, i64) {
    let node_id = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES ('dn','x','online','1.2.3.4',10000,65535)",
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
            "node_id": node_id, "name": "r", "protocol": "tcp", "listen_port": 20000,
            "target_host": "9.9.9.9", "target_port": 443
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create rule: {body}");
    (node_id, body["id"].as_i64().unwrap())
}

#[tokio::test]
async fn diagnose_rule_offline_node_reports_not_dispatched() {
    let app = make_app().await.unwrap();
    let (_node, rule_id) = make_node_and_rule(&app).await;
    // 无 agent 订阅 → dispatch 失败 → dispatched:false。
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/diagnose"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let segs = body["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 1, "非隧道规则一段");
    assert_eq!(segs[0]["dispatched"], false);
    assert_eq!(segs[0]["target"], "9.9.9.9:443");
}

#[tokio::test]
async fn diagnose_rule_round_trip_with_mock_agent() {
    let app = make_app().await.unwrap();
    let (node_id, rule_id) = make_node_and_rule(&app).await;

    // mock agent:订阅该节点的命令通道(使 dispatch 成功)。
    let mut rx = app.state.dispatcher.subscribe(node_id).0;

    // 发起诊断(后台),它会 dispatch Probe 并等回报。
    let app_clone = app.app.clone();
    let token = app.admin_token.clone();
    let diag = tokio::spawn(async move {
        let req = auth_req(
            Method::POST,
            &format!("/api/rules/{rule_id}/diagnose"),
            &token,
            None,
        )
        .unwrap();
        send(app_clone, req).await.unwrap()
    });

    // 读到 Probe 命令,取 probe_id,模拟 Agent 回报成功。
    let cmd = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .expect("应收到 Probe 命令")
        .expect("通道未关");
    let probe_id = match cmd.body {
        Some(Body::Probe(p)) => {
            assert_eq!(p.target_host, "9.9.9.9");
            assert_eq!(p.target_port, 443);
            p.probe_id
        }
        other => panic!("expected Probe, got {other:?}"),
    };
    app.state.resolve_probe(ProbeResult {
        probe_id,
        reachable: true,
        avg_latency_ms: 12.5,
        loss_pct: 0.0,
        error: String::new(),
    });

    let (status, body) = diag.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let seg = &body["segments"][0];
    assert_eq!(seg["dispatched"], true);
    assert_eq!(seg["reachable"], true);
    assert_eq!(seg["avg_latency_ms"], 12.5);
    assert_eq!(seg["loss_pct"], 0.0);
}

#[tokio::test]
async fn diagnose_rule_requires_ownership() {
    let app = make_app().await.unwrap();
    let (_node, rule_id) = make_node_and_rule(&app).await; // admin 的规则
    let (_uid, user_token) = common::make_user_token(&app, "diaguser", "password123")
        .await
        .unwrap();
    // 普通用户诊断别人的规则 → 404(不泄漏存在性)。
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/diagnose"),
        &user_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}
