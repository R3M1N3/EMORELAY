mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;

#[tokio::test]
async fn node_full_crud_cycle() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    // create
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({
            "name": "test-node",
            "region": "test",
            "public_ip": "1.2.3.4",
            "grpc_endpoint": "http://test:50051",
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let node_id = body["node"]["id"].as_i64().expect("node.id");
    let token_returned = body["agent_token"].as_str().expect("agent_token");
    assert!(!token_returned.is_empty(), "agent_token must be non-empty");
    assert_eq!(body["node"]["name"], "test-node");
    // 默认端口池 1-65535
    assert_eq!(body["node"]["port_pool_min"], 1);
    assert_eq!(body["node"]["port_pool_max"], 65535);

    // get
    let req = auth_req(Method::GET, &format!("/api/nodes/{node_id}"), t, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "test-node");

    // list 含该 node
    let req = auth_req(Method::GET, "/api/nodes", t, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["id"] == node_id));

    // patch
    let req = auth_req(
        Method::PATCH,
        &format!("/api/nodes/{node_id}"),
        t,
        Some(json!({ "region": "updated", "port_pool_min": 1000, "port_pool_max": 2000 })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "patch failed: {body}");
    assert_eq!(body["region"], "updated");
    assert_eq!(body["port_pool_min"], 1000);

    // delete (软删)
    let req = auth_req(Method::DELETE, &format!("/api/nodes/{node_id}"), t, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    // get 软删后 → 404
    let req = auth_req(Method::GET, &format!("/api/nodes/{node_id}"), t, None).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_node_without_auth_returns_401() {
    let app = make_app().await.unwrap();
    let req = Request::post("/api/nodes")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({ "name": "x" })).unwrap(),
        ))
        .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_node_with_inverted_port_pool_returns_400() {
    let app = make_app().await.unwrap();
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "bad", "port_pool_min": 100, "port_pool_max": 50 })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_duplicate_node_name_returns_400() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let req =
        auth_req(Method::POST, "/api/nodes", t, Some(json!({ "name": "dup" }))).unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);

    let req =
        auth_req(Method::POST, "/api/nodes", t, Some(json!({ "name": "dup" }))).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("已存在"));
}

#[tokio::test]
async fn node_stats_returns_empty_series_initially() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;
    let req = auth_req(Method::POST, "/api/nodes", t, Some(json!({ "name": "n" }))).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    let req = auth_req(
        Method::GET,
        &format!("/api/nodes/{node_id}/stats"),
        t,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["current"]["status"], "unknown");
    assert_eq!(body["series"].as_array().unwrap().len(), 0);
}

// ============ P4: 普通用户净化视图 + 服务端搜索 ============

#[tokio::test]
async fn normal_user_gets_sanitized_node_list() {
    let app = make_app().await.unwrap();
    sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, region, public_ip, grpc_endpoint, \
                            cpu_usage, rx_bytes_total, agent_version, status) \
         VALUES ('n1', 'x', 'HK', '1.2.3.4', 'https://internal:7001', 55.5, 999, '9.9.9', 'online')",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    let (_uid, token) = common::make_user_token(&app, "nuser", "password123").await.unwrap();

    let req = auth_req(Method::GET, "/api/nodes?page_size=20", &token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "user 应可读节点列表: {body}");
    let item = &body["items"][0];
    // 净化:运维与控制面字段抹掉
    assert_eq!(item["grpc_endpoint"], "");
    assert_eq!(item["agent_version"], "");
    assert_eq!(item["cpu_usage"], 0.0);
    assert_eq!(item["rx_bytes_total"], 0);
    // 自助建规则所需字段保留
    assert_eq!(item["name"], "n1");
    assert_eq!(item["region"], "HK");
    assert_eq!(item["public_ip"], "1.2.3.4");
    assert_eq!(item["status"], "online");
    assert!(item.get("port_pool_min").is_some());

    // 单条 GET 同样净化
    let nid = item["id"].as_i64().unwrap();
    let req = auth_req(Method::GET, &format!("/api/nodes/{nid}"), &token, None).unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["grpc_endpoint"], "");

    // admin 视图不净化
    let req = auth_req(Method::GET, "/api/nodes?page_size=20", &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["items"][0]["grpc_endpoint"], "https://internal:7001");
    assert_eq!(body["items"][0]["agent_version"], "9.9.9");
}

#[tokio::test]
async fn normal_user_still_cannot_mutate_nodes() {
    let app = make_app().await.unwrap();
    let (_uid, token) = common::make_user_token(&app, "nuser2", "password123").await.unwrap();
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &token,
        Some(json!({ "name": "evil" })),
    )
    .unwrap();
    let (status, _) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn nodes_list_server_side_search() {
    let app = make_app().await.unwrap();
    for (name, region, ip) in [("hk-a", "HK", "1.1.1.1"), ("jp-b", "JP", "2.2.2.2")] {
        sqlx::query("INSERT INTO nodes (name, agent_token_hash, region, public_ip) VALUES (?, 'x', ?, ?)")
            .bind(name).bind(region).bind(ip)
            .execute(&app.state.pool).await.unwrap();
    }
    // 命中 name
    let req = auth_req(Method::GET, "/api/nodes?search=hk-", &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 1, "{body}");
    assert_eq!(body["items"][0]["name"], "hk-a");
    // 命中 IP
    let req = auth_req(Method::GET, "/api/nodes?search=2.2.2", &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["name"], "jp-b");
    // LIKE 通配符被转义:'%' 不应匹配所有
    let req = auth_req(Method::GET, "/api/nodes?search=%25", &app.admin_token, None).unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(body["total"], 0, "通配符必须按字面量处理: {body}");
}
