// crates/panel-server/tests/stats_retention_sweeper.rs
//! 直接调 tick(确定性),不等 interval。模式同 user_quota_sweeper.rs。
mod common;

use panel_server::sweeper::stats_retention::retention_tick_once;

#[tokio::test]
async fn retention_deletes_old_buckets_keeps_recent() {
    let app = common::make_app().await.unwrap();
    // 种 node + rule(满足 FK)
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('rn', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
         VALUES (?, 1, 'rr', 'tcp', '0.0.0.0', 21001, '1.2.3.4', 443)",
    )
    .bind(app.admin_user_id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    // 旧桶(40 天前)与新桶(现在),rule_stats 与 node_stats 各一对
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes) \
         VALUES (1, datetime('now','-40 days'), 1), (1, datetime('now'), 2)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO node_stats (node_id, bucket_at, rx_bytes) \
         VALUES (1, datetime('now','-40 days'), 1), (1, datetime('now'), 2)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let deleted = retention_tick_once(&app.state).await.unwrap();
    assert_eq!(deleted, 2, "默认保留 30 天:每表删 1 行旧桶");

    let rule_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rule_stats")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let node_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM node_stats")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!((rule_left, node_left), (1, 1));

    // 幂等:再跑一次无可删。
    assert_eq!(retention_tick_once(&app.state).await.unwrap(), 0);
}

#[tokio::test]
async fn retention_respects_settings_value() {
    let app = common::make_app().await.unwrap();
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('rn2', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO node_stats (node_id, bucket_at, rx_bytes) \
         VALUES (1, datetime('now','-10 days'), 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    // 保留 7 天 → 10 天前的桶应被删
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ('stats_retention_days', '7') \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let deleted = retention_tick_once(&app.state).await.unwrap();
    assert_eq!(deleted, 1);
}

#[tokio::test]
async fn retention_ignores_invalid_setting_and_keeps_default() {
    let app = common::make_app().await.unwrap();
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('rn3', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    // 20 天前的桶:默认 30 天内,不应被删——即使设置值非法(0)也回落默认。
    sqlx::query(
        "INSERT INTO node_stats (node_id, bucket_at, rx_bytes) \
         VALUES (1, datetime('now','-20 days'), 1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO system_settings (key, value) VALUES ('stats_retention_days', '0') \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&app.state.pool)
    .await
    .unwrap();

    let deleted = retention_tick_once(&app.state).await.unwrap();
    assert_eq!(deleted, 0, "非法配置回落默认 30 天,20 天前的桶保留");
}
