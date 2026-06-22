mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, make_user_token, send, TestApp};
use serde_json::json;

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

async fn create_rule_as(
    app: &TestApp,
    token: &str,
    node_id: i64,
    name: &str,
    port: u16,
) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/rules",
        token,
        Some(json!({
            "node_id": node_id,
            "name": name,
            "protocol": "tcp",
            "listen_port": port,
            "target_host": "1.2.3.4",
            "target_port": 80,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create_rule failed: {body}");
    body["id"].as_i64().unwrap()
}

#[tokio::test]
async fn user_only_sees_own_rules_in_list() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let (bob_id, bob_token) = make_user_token(&app, "bob", "bob-password").await.unwrap();
    // P7: 普通用户建规则需节点授权。
    common::grant_node(&app, alice_id, node_id).await;
    common::grant_node(&app, bob_id, node_id).await;

    let alice_rule = create_rule_as(&app, &alice_token, node_id, "alice-rule", 30001).await;
    let bob_rule = create_rule_as(&app, &bob_token, node_id, "bob-rule", 30002).await;
    let admin_rule = create_rule_as(&app, &app.admin_token, node_id, "admin-rule", 30003).await;

    // alice 只看到自己的规则
    let req = auth_req(Method::GET, "/api/rules", &alice_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], alice_rule);

    // admin 看到全部
    let req = auth_req(Method::GET, "/api/rules", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let admin_items = body["items"].as_array().unwrap();
    assert_eq!(admin_items.len(), 3);
    let ids: Vec<i64> = admin_items.iter().map(|n| n["id"].as_i64().unwrap()).collect();
    assert!(ids.contains(&alice_rule));
    assert!(ids.contains(&bob_rule));
    assert!(ids.contains(&admin_rule));
}

#[tokio::test]
async fn user_cannot_get_other_rule_returns_404() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (_, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let admin_rule = create_rule_as(&app, &app.admin_token, node_id, "admin-rule", 30010).await;

    let req = auth_req(
        Method::GET,
        &format!("/api/rules/{admin_rule}"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    // 用 NotFound 不暴露存在性。
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_cannot_modify_other_rule() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (_, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let admin_rule = create_rule_as(&app, &app.admin_token, node_id, "admin-rule", 30020).await;

    // PATCH
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{admin_rule}"),
        &alice_token,
        Some(json!({ "name": "hijacked" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);

    // disable
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{admin_rule}/disable"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);

    // restart
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{admin_rule}/restart"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);

    // delete
    let req = auth_req(
        Method::DELETE,
        &format!("/api/rules/{admin_rule}"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_can_manage_own_rule_full_cycle() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;
    let rule_id = create_rule_as(&app, &alice_token, node_id, "alice-rule", 30030).await;

    // GET own
    let req = auth_req(
        Method::GET,
        &format!("/api/rules/{rule_id}"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "alice-rule");

    // PATCH own
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &alice_token,
        Some(json!({ "name": "alice-renamed" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "alice-renamed");

    // disable own
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/disable"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // delete own
    let req = auth_req(
        Method::DELETE,
        &format!("/api/rules/{rule_id}"),
        &alice_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn audit_logs_actor_ip_populated_via_xff_header() {
    use axum::body::Body;
    use axum::http::Request;

    let app = make_app().await.unwrap();

    // 登录请求带 X-Forwarded-For,验证 audit_logs.actor_ip 落库。
    let req = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "203.0.113.5, 10.0.0.1")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "username": "admin",
                "password": "admin-test-password"
            }))
            .unwrap(),
        ))
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // 查最近的 auth.login success,actor_ip 应当取 XFF 第一段。
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT actor_ip FROM audit_logs \
         WHERE action = 'auth.login' AND result = 'success' \
         ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&app.state.pool)
    .await
    .unwrap();
    let (actor_ip,) = row.expect("expected at least one auth.login audit row");
    assert_eq!(actor_ip.as_deref(), Some("203.0.113.5"));
}

// ============ P4: 规则归属与权限收紧 ============

#[tokio::test]
async fn admin_can_assign_rule_owner() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, _) = make_user_token(&app, "alice", "alice-password").await.unwrap();

    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "for-alice", "protocol": "tcp",
            "listen_port": 30030, "target_host": "1.2.3.4", "target_port": 80,
            "user_id": alice_id,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user_id"], alice_id);
    assert_eq!(body["user_name"], "alice");

    // 不存在的归属用户 → 400
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "ghost-owner", "protocol": "tcp",
            "listen_port": 30031, "target_host": "1.2.3.4", "target_port": 80,
            "user_id": 99999,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn user_cannot_assign_other_owner_or_profile_or_tunnel() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;

    // 指定他人归属 → 400
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &alice_token,
        Some(json!({
            "node_id": node_id, "name": "x1", "protocol": "tcp",
            "listen_port": 30040, "target_host": "1.2.3.4", "target_port": 80,
            "user_id": app.admin_user_id,
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 自配限速 → 400
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &alice_token,
        Some(json!({
            "node_id": node_id, "name": "x2", "protocol": "tcp",
            "listen_port": 30041, "target_host": "1.2.3.4", "target_port": 80,
            "bandwidth_profile_id": 1,
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 挂未授权隧道 → 400(P7 起隧道按授权放开,未授权仍拒)
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &alice_token,
        Some(json!({
            "node_id": node_id, "name": "x3", "protocol": "tcp",
            "listen_port": 30042, "target_host": "1.2.3.4", "target_port": 80,
            "tunnel_id": 1,
        })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 显式传自己 id 放行(等价于不传)——覆盖 Some(uid)+非 admin+uid==sub 分支。
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &alice_token,
        Some(json!({
            "node_id": node_id, "name": "self-ok", "protocol": "tcp",
            "listen_port": 30043, "target_host": "1.2.3.4", "target_port": 80,
            "user_id": alice_id,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user_id"], alice_id);
}

#[tokio::test]
async fn user_cannot_clear_admin_bandwidth_profile() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();

    // admin 建限速档并把规则归 alice + 挂限速。
    let req = auth_req(
        Method::POST,
        "/api/bandwidth-profiles",
        &app.admin_token,
        Some(json!({ "name": "cap10", "bandwidth_mbps": 10 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let pid = body["id"].as_i64().unwrap();

    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "capped", "protocol": "tcp",
            "listen_port": 30050, "target_host": "1.2.3.4", "target_port": 80,
            "user_id": alice_id, "bandwidth_profile_id": pid,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule_id = body["id"].as_i64().unwrap();

    // alice 尝试解除限速(bandwidth_profile_id=0) → 400;改名等普通字段仍可。
    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &alice_token,
        Some(json!({ "bandwidth_profile_id": 0 })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let req = auth_req(
        Method::PATCH,
        &format!("/api/rules/{rule_id}"),
        &alice_token,
        Some(json!({ "name": "renamed-by-alice" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["bandwidth_profile_id"], pid, "限速关联不受改名影响");
}

#[tokio::test]
async fn rule_list_includes_owner_username_for_admin() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;
    create_rule_as(&app, &alice_token, node_id, "alice-named", 30060).await;

    let req = auth_req(Method::GET, "/api/rules", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "alice-named")
        .expect("rule present");
    assert_eq!(item["user_name"], "alice");
}

// ============ C1: 规则列表 user_id / enabled 筛选 + 越权回归 ============

#[tokio::test]
async fn admin_filter_by_user_id() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let (bob_id, bob_token) = make_user_token(&app, "bob", "bob-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;
    common::grant_node(&app, bob_id, node_id).await;
    let alice_rule = create_rule_as(&app, &alice_token, node_id, "alice-rule", 30071).await;
    create_rule_as(&app, &bob_token, node_id, "bob-rule", 30072).await;

    // admin 按 ?user_id=alice 筛选 → 只返回 alice 的规则。
    let req = auth_req(
        Method::GET,
        &format!("/api/rules?user_id={alice_id}"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "应只返回 alice 的规则: {body}");
    assert_eq!(items[0]["id"], alice_rule);
}

#[tokio::test]
async fn admin_filter_by_enabled() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let r_enabled = create_rule_as(&app, &app.admin_token, node_id, "r-enabled", 30081).await;
    let r_disabled = create_rule_as(&app, &app.admin_token, node_id, "r-disabled", 30082).await;
    // 禁用其中一条。
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{r_disabled}/disable"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    // ?enabled=true → 只启用的。
    let req = auth_req(Method::GET, "/api/rules?enabled=true", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "enabled=true 应只 1 条: {body}");
    assert_eq!(items[0]["id"], r_enabled);

    // ?enabled=false → 只禁用的(验证 false 也能下发筛选,不被当作「不筛选」)。
    let req = auth_req(Method::GET, "/api/rules?enabled=false", &app.admin_token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "enabled=false 应只 1 条: {body}");
    assert_eq!(items[0]["id"], r_disabled);
}

#[tokio::test]
async fn user_filter_user_id_cannot_escalate() {
    let app = make_app().await.unwrap();
    let node_id = make_node(&app).await;
    let (alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let (bob_id, bob_token) = make_user_token(&app, "bob", "bob-password").await.unwrap();
    common::grant_node(&app, alice_id, node_id).await;
    common::grant_node(&app, bob_id, node_id).await;
    create_rule_as(&app, &alice_token, node_id, "alice-rule", 30091).await;
    let bob_rule = create_rule_as(&app, &bob_token, node_id, "bob-rule", 30092).await;

    // 越权回归:bob 传 ?user_id=alice 试图越权看 alice 的规则。
    // 后端 restrict_user_id(=bob) 与 filter user_id(=alice)两个 user_id=? 条件并存,
    // 两值不等 → 空集,绝不泄露 alice 的规则。这是双 user_id 兜底的安全断言。
    let req = auth_req(
        Method::GET,
        &format!("/api/rules?user_id={alice_id}"),
        &bob_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 0, "bob 用 ?user_id=alice 越权应得空集,不泄露: {body}");

    // bob 传 ?user_id=自己 → 正常看到自己的(filter 与 restrict 一致)。
    let req = auth_req(
        Method::GET,
        &format!("/api/rules?user_id={bob_id}"),
        &bob_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], bob_rule);
}
