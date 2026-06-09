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
        .contains("already exists"));
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
