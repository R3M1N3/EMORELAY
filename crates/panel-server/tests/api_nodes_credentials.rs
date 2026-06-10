mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn create_node_returns_four_credential_blocks() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({ "name": "hk-relay-01" })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(body["agent_token"].as_str().unwrap().len() >= 16);
    assert!(body["ca_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(body["client_cert_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    let key = body["client_key_pem"].as_str().unwrap();
    assert!(key.contains("BEGIN PRIVATE KEY") || key.contains("BEGIN EC PRIVATE KEY"));

    let node_id = body["node"]["id"].as_i64().unwrap();

    let (serial, fp): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT cert_serial, cert_fingerprint FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(serial.is_some() && fp.is_some(), "证书元数据必须落库");
    let dump: Vec<(String,)> = sqlx::query_as("SELECT cert_serial FROM nodes WHERE id = ?")
        .bind(node_id)
        .fetch_all(&app.state.pool)
        .await
        .unwrap();
    assert!(!dump[0].0.contains("PRIVATE KEY"));
}
