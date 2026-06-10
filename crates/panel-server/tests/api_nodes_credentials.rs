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

#[tokio::test]
async fn revoke_credentials_rotates_and_revokes_old() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(Method::POST, "/api/nodes", &app.admin_token,
        Some(json!({ "name": "rev-node" }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let (old_fp,): (String,) = sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();

    let req = common::auth_req(Method::POST, &format!("/api/nodes/{node_id}/revoke-credentials"),
        &app.admin_token, None).unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["client_cert_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(body["ca_pem"].as_str().unwrap().contains("BEGIN CERTIFICATE"));
    assert!(app.state.crl.is_revoked(&old_fp), "旧证书必须进 CRL");

    let (new_fp,): (String,) = sqlx::query_as("SELECT cert_fingerprint FROM nodes WHERE id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();
    assert_ne!(old_fp, new_fp);

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'node.credentials_revoked' AND target_id = ?")
        .bind(node_id).fetch_one(&app.state.pool).await.unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn revoke_requires_admin() {
    let app = common::make_app().await.unwrap();
    let req = common::auth_req(Method::POST, "/api/nodes", &app.admin_token,
        Some(json!({ "name": "n2" }))).unwrap();
    let (_, body) = common::send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();
    let (_uid, token) = common::make_user_token(&app, "nonadmin", "password123").await.unwrap();
    let req = common::auth_req(Method::POST, &format!("/api/nodes/{node_id}/revoke-credentials"),
        &token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
