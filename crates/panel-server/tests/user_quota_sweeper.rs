// crates/panel-server/tests/user_quota_sweeper.rs
//! 直接调 tick 函数测试(确定性),不依赖真实 interval。
mod common;

use panel_server::sweeper::user_quota::{expiry_tick_once, quota_tick_once};

/// 建 node + 指定 user 名下两条 enabled 规则,返回 rule ids。
async fn seed_rules(app: &common::TestApp, user_id: i64) -> (i64, i64) {
    sqlx::query("INSERT INTO nodes (name, agent_token_hash) VALUES ('swn', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for (i, port) in [(1, 21001_i64), (2, 21002)] {
        let res = sqlx::query(
            "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port) \
             VALUES (?, 1, ?, 'tcp', '0.0.0.0', ?, '1.2.3.4', 443)",
        )
        .bind(user_id)
        .bind(format!("swr{i}"))
        .bind(port)
        .execute(&app.state.pool)
        .await
        .unwrap();
        ids.push(res.last_insert_rowid());
    }
    (ids[0], ids[1])
}

async fn enabled_of(app: &common::TestApp, rule_id: i64) -> i64 {
    sqlx::query_scalar("SELECT enabled FROM forward_rules WHERE id = ?")
        .bind(rule_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn expiry_tick_disables_rules_of_expired_user() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "expuser", "password123").await.unwrap();
    sqlx::query("UPDATE users SET expires_at = '2020-01-01 00:00:00' WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, r2) = seed_rules(&app, uid).await;

    let hit = expiry_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 1, "应命中 1 个过期用户");
    assert_eq!(enabled_of(&app, r1).await, 0);
    assert_eq!(enabled_of(&app, r2).await, 0);

    // audit 聚合一条
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'user.expired_auto_disable_rules' AND target_id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    // 幂等:再跑一次不重复触发(规则已 disabled,无 enabled=1 命中)
    let hit2 = expiry_tick_once(&app.state).await.unwrap();
    assert_eq!(hit2, 0);
}

#[tokio::test]
async fn quota_tick_refreshes_cache_then_disables_over_limit() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "quotau", "password123").await.unwrap();
    sqlx::query("UPDATE users SET traffic_limit_bytes_30d = 100 WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, _r2) = seed_rules(&app, uid).await;
    // 30 天窗口内的 rule_stats:rx+tx = 150 > 100
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) \
         VALUES (?, datetime('now'), 100, 50)",
    )
    .bind(r1)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let hit = quota_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 1, "应命中 1 个超额用户");

    let (cached, calc_at): (i64, Option<String>) = sqlx::query_as(
        "SELECT period_used_bytes_cached, period_used_calculated_at FROM users WHERE id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(cached, 150, "cache 必须先刷新再判定");
    assert!(calc_at.is_some());
    assert_eq!(enabled_of(&app, r1).await, 0);

    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE action = 'user.quota_exceeded_auto_disable_rules' AND target_id = ?",
    )
    .bind(uid)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn quota_tick_ignores_unlimited_and_under_limit_users() {
    let app = common::make_app().await.unwrap();
    // admin 无 limit;再建一个 limit 充足的用户
    let (uid, _) = common::make_user_token(&app, "underu", "password123").await.unwrap();
    sqlx::query("UPDATE users SET traffic_limit_bytes_30d = 1000000 WHERE id = ?")
        .bind(uid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let (r1, _) = seed_rules(&app, uid).await;
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) VALUES (?, datetime('now'), 10, 10)",
    )
    .bind(r1)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let hit = quota_tick_once(&app.state).await.unwrap();
    assert_eq!(hit, 0);
    assert_eq!(enabled_of(&app, r1).await, 1);
}
