//! A 组:用户级「转发条数」配额(forward_rules_quota)创建时校验。
//! per-(用户,隧道) 上限的 COUNT 校验与全局同构,存储/查询链路由 models::grant 单测覆盖。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, make_user_token, send, TestApp};
use serde_json::{json, Value};

async fn make_node(app: &TestApp) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "n1" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    body["node"]["id"].as_i64().unwrap()
}

async fn set_user_quota(app: &TestApp, user_id: i64, quota: i64) {
    let req = auth_req(
        Method::PATCH,
        &format!("/api/users/{user_id}"),
        &app.admin_token,
        Some(json!({ "forward_rules_quota": quota })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "set quota failed: {body}");
}

fn rule_body(node_id: i64, name: &str, port: u16) -> Value {
    json!({
        "node_id": node_id, "name": name, "protocol": "tcp",
        "listen_port": port, "target_host": "1.2.3.4", "target_port": 80,
    })
}

async fn create_rule(app: &TestApp, token: &str, node_id: i64, name: &str, port: u16) -> (StatusCode, Value) {
    let req = auth_req(Method::POST, "/api/rules", token, Some(rule_body(node_id, name, port))).unwrap();
    send(app.app.clone(), req).await.unwrap()
}

#[tokio::test]
async fn user_global_forward_rules_quota_enforced() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;
    set_user_quota(&app, alice_id, 2).await;

    // 配额内的前两条放行。
    let (s1, b1) = create_rule(&app, &alice_token, node_id, "r1", 31001).await;
    assert_eq!(s1, StatusCode::OK, "rule1 should pass: {b1}");
    let (s2, b2) = create_rule(&app, &alice_token, node_id, "r2", 31002).await;
    assert_eq!(s2, StatusCode::OK, "rule2 should pass: {b2}");

    // 第三条超限 → 400 + 中文上限提示。
    let (s3, b3) = create_rule(&app, &alice_token, node_id, "r3", 31003).await;
    assert_eq!(s3, StatusCode::BAD_REQUEST, "rule3 must be rejected over quota: {b3}");
    assert!(
        b3["message"].as_str().unwrap_or_default().contains("上限"),
        "应返回中文超限提示, got {b3}"
    );

    // 删一条后名额回收,可再建(软删不计入 COUNT)。
    let req = auth_req(Method::GET, "/api/rules", &alice_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let rid = body["items"][0]["id"].as_i64().unwrap();
    let req = auth_req(Method::DELETE, &format!("/api/rules/{rid}"), &alice_token, None).unwrap();
    let (sd, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(sd, StatusCode::OK);
    let (s4, b4) = create_rule(&app, &alice_token, node_id, "r-again", 31004).await;
    assert_eq!(s4, StatusCode::OK, "after delete, under quota again: {b4}");
}

#[tokio::test]
async fn admin_rules_not_limited_by_quota() {
    // admin 自身规则不受配额限制(admin 用户 forward_rules_quota 恒 NULL)。
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    for (i, port) in [32001u16, 32002, 32003].iter().enumerate() {
        let (s, b) = create_rule(&app, &app.admin_token, node_id, &format!("a{i}"), *port).await;
        assert_eq!(s, StatusCode::OK, "admin rule {i} should pass: {b}");
    }
}

async fn seed_online_node(app: &TestApp, name: &str) -> i64 {
    let req = auth_req(Method::POST, "/api/nodes", &app.admin_token, Some(json!({ "name": name }))).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let id = body["node"]["id"].as_i64().unwrap();
    // 隧道创建要求各跳节点在线,且第 2 跳起需公网 IP(中继互联地址)。直接置 online + 公网 IP。
    sqlx::query("UPDATE nodes SET status = 'online', public_ip = '198.51.100.' || id WHERE id = ?")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn per_tunnel_forward_limit_enforced_and_roundtrips() {
    let app = make_app().await.unwrap();
    let entry = seed_online_node(&app, "entry").await;
    let exit = seed_online_node(&app, "exit").await;

    // 建双跳隧道(entry = node_ids[0])。
    let req = auth_req(
        Method::POST,
        "/api/tunnels",
        &app.admin_token,
        Some(json!({ "name": "t1", "transport": "tcp", "node_ids": [entry, exit] })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "create tunnel: {body}");
    let tid = body["id"].as_i64().unwrap();

    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();

    // 授权该隧道给 alice 并设 per-隧道上限 = 1(走真实 user update 链路 tunnel_forward_limits)。
    let req = auth_req(
        Method::PATCH,
        &format!("/api/users/{alice_id}"),
        &app.admin_token,
        Some(json!({
            "granted_tunnel_ids": [tid],
            "tunnel_forward_limits": [{ "tunnel_id": tid, "limit": 1 }],
        })),
    )
    .unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "grant tunnel with limit: {body}");

    // grants 端点回显该上限。
    let req = auth_req(Method::GET, &format!("/api/users/{alice_id}/grants"), &app.admin_token, None).unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "grants: {body}");
    let limits = body["tunnel_forward_limits"].as_array().unwrap();
    assert_eq!(limits.len(), 1, "应回显一条隧道上限: {body}");
    assert_eq!(limits[0]["tunnel_id"], tid);
    assert_eq!(limits[0]["limit"], 1);

    // alice 在该隧道建第 1 条规则(listen_port 留空走端口池自动分配)→ OK。
    let mk = |name: &str| {
        json!({
            "node_id": entry, "name": name, "protocol": "tcp",
            "target_host": "1.2.3.4", "target_port": 80, "tunnel_id": tid,
        })
    };
    let req = auth_req(Method::POST, "/api/rules", &alice_token, Some(mk("t-r1"))).unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::OK, "1st tunnel rule should pass: {body}");

    // 第 2 条触 per-隧道上限 → 400 + 中文提示。
    let req = auth_req(Method::POST, "/api/rules", &alice_token, Some(mk("t-r2"))).unwrap();
    let (s, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(s, StatusCode::BAD_REQUEST, "2nd tunnel rule must hit per-tunnel limit: {body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("隧道"),
        "应返回隧道上限中文提示, got {body}"
    );
}

#[tokio::test]
async fn quota_zero_or_unset_means_unlimited() {
    // 未设配额(NULL)= 不限:建多条均放行;PATCH 设 0 亦视为清除(回不限)。
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;

    for (i, port) in [33001u16, 33002, 33003].iter().enumerate() {
        let (s, b) = create_rule(&app, &alice_token, node_id, &format!("u{i}"), *port).await;
        assert_eq!(s, StatusCode::OK, "unset quota should be unlimited, rule {i}: {b}");
    }

    // 设 2 再清 0:第 4、5 条应放行(0 = 清除回不限)。
    set_user_quota(&app, alice_id, 0).await;
    let (s, b) = create_rule(&app, &alice_token, node_id, "u3", 33004).await;
    assert_eq!(s, StatusCode::OK, "quota=0 cleared → unlimited: {b}");
}
