// crates/panel-server/tests/notify_webhook.rs
//! 用一次性本地 TCP 接收器验证 webhook POST 行为(不引入 wiremock 依赖)。
mod common;

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 起一个收一条 HTTP 请求就关的接收器,返回 (url, body oneshot)。
async fn one_shot_receiver() -> (String, tokio::sync::oneshot::Receiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let mut data = Vec::new();
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if let Some(pos) = header_end(&data) {
                let head = String::from_utf8_lossy(&data[..pos]).to_string();
                let cl: usize = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if data.len() >= pos + 4 + cl {
                    let body = String::from_utf8_lossy(&data[pos + 4..pos + 4 + cl]).to_string();
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                        .await;
                    let _ = tx.send(body);
                    break;
                }
            }
        }
    });
    (format!("http://{addr}/hook"), rx)
}

#[tokio::test]
async fn webhook_posts_event_json_when_configured() {
    let app = common::make_app().await.unwrap();
    let (url, rx) = one_shot_receiver().await;
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ('notify_webhook_url', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(&url)
    .execute(&app.state.pool)
    .await
    .unwrap();

    panel_server::notify::spawn_send(
        app.state.clone(),
        "node.offline",
        serde_json::json!({ "node_id": 1, "name": "n1" }),
    );

    let body = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("webhook 应在 5s 内送达")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["event"], "node.offline");
    assert_eq!(v["data"]["node_id"], 1);
    assert_eq!(v["data"]["name"], "n1");
    assert!(
        v["occurred_at"].as_str().unwrap().contains('T'),
        "occurred_at 应为 RFC3339: {v}"
    );
}

#[tokio::test]
async fn webhook_noop_when_unconfigured() {
    let app = common::make_app().await.unwrap();
    // 未配置 → 静默返回,不 panic(fire-and-forget,给点时间让 task 跑完)。
    panel_server::notify::spawn_send(app.state.clone(), "node.offline", serde_json::json!({}));
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn settings_accept_and_validate_webhook_url() {
    let app = common::make_app().await.unwrap();
    use axum::http::Method;
    // 合法 URL 接受
    let req = common::auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({ "settings": { "notify_webhook_url": "https://example.com/hook" } })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["notify_webhook_url"], "https://example.com/hook");

    // 非法 scheme 拒绝
    let req = common::auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({ "settings": { "notify_webhook_url": "ftp://example.com/x" } })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);

    // 空串 = 关闭,接受
    let req = common::auth_req(
        Method::PATCH,
        "/api/system/settings",
        &app.admin_token,
        Some(serde_json::json!({ "settings": { "notify_webhook_url": "" } })),
    )
    .unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);
}
