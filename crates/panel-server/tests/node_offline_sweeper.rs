// crates/panel-server/tests/node_offline_sweeper.rs
//! 直接调 tick(确定性),不等 interval。模式同 user_quota_sweeper.rs。
mod common;

use panel_server::sweeper::node_offline::offline_tick_once;

async fn seed_node(
    app: &common::TestApp,
    name: &str,
    status: &str,
    last_seen_modifier: Option<&str>,
) -> i64 {
    let res = match last_seen_modifier {
        Some(m) => sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, last_seen_at) \
             VALUES (?, 'x', ?, datetime('now', ?))",
        )
        .bind(name)
        .bind(status)
        .bind(m)
        .execute(&app.state.pool)
        .await
        .unwrap(),
        None => sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status) VALUES (?, 'x', ?)",
        )
        .bind(name)
        .bind(status)
        .execute(&app.state.pool)
        .await
        .unwrap(),
    };
    res.last_insert_rowid()
}

async fn status_of(app: &common::TestApp, id: i64) -> String {
    sqlx::query_scalar("SELECT status FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn stale_online_node_marked_offline() {
    let app = common::make_app().await.unwrap();
    let stale = seed_node(&app, "stale", "online", Some("-300 seconds")).await;
    let fresh = seed_node(&app, "fresh", "online", Some("-10 seconds")).await;
    let never = seed_node(&app, "never", "unknown", Some("-300 seconds")).await;
    // online 但 last_seen_at 为 NULL(异常态)也应翻 offline。
    let null_seen = seed_node(&app, "nullseen", "online", None).await;

    let flipped = offline_tick_once(&app.state).await.unwrap();
    assert_eq!(flipped, 2, "超时的 online 节点 + NULL last_seen 的 online 节点");

    assert_eq!(status_of(&app, stale).await, "offline");
    assert_eq!(status_of(&app, fresh).await, "online", "心跳新鲜的不动");
    assert_eq!(status_of(&app, never).await, "unknown", "从未上线的不动");
    assert_eq!(status_of(&app, null_seen).await, "offline");

    // audit 落了聚合记录
    let cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'node.offline_detected'",
    )
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(cnt, 1);

    // 幂等:已 offline 不再命中。
    assert_eq!(offline_tick_once(&app.state).await.unwrap(), 0);
}

#[tokio::test]
async fn soft_deleted_node_not_flipped() {
    let app = common::make_app().await.unwrap();
    let nid = seed_node(&app, "ghost", "online", Some("-300 seconds")).await;
    sqlx::query("UPDATE nodes SET deleted_at = datetime('now') WHERE id = ?")
        .bind(nid)
        .execute(&app.state.pool)
        .await
        .unwrap();

    assert_eq!(offline_tick_once(&app.state).await.unwrap(), 0, "软删节点不参与掉线判定");
    assert_eq!(status_of(&app, nid).await, "online", "软删行原样保留");
}

#[tokio::test]
async fn offline_flip_sends_webhook() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let app = common::make_app().await.unwrap();
    // 一次性接收器(与 notify_webhook.rs 同构,这里只断言 event 名)。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..pos]).to_string();
                let cl: usize = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if data.len() >= pos + 4 + cl {
                    let body =
                        String::from_utf8_lossy(&data[pos + 4..pos + 4 + cl]).to_string();
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                        .await;
                    let _ = tx.send(body);
                    break;
                }
            }
        }
    });
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ('notify_webhook_url', ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(format!("http://{addr}/hook"))
    .execute(&app.state.pool)
    .await
    .unwrap();

    let nid = seed_node(&app, "whk", "online", Some("-300 seconds")).await;
    assert_eq!(offline_tick_once(&app.state).await.unwrap(), 1);

    let body = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("webhook 应送达")
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["event"], "node.offline");
    assert_eq!(v["data"]["node_id"], nid);
    assert_eq!(v["data"]["name"], "whk");
}
