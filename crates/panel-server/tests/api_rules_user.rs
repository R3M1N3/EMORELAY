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
    let (_alice_id, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
    let (_bob_id, bob_token) = make_user_token(&app, "bob", "bob-password").await.unwrap();

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
    let (_, alice_token) = make_user_token(&app, "alice", "alice-password").await.unwrap();
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
