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

/// 在节点 1 上建一条挂隧道 tid 的规则,返回 rule_id;隧道计费参数由调用方预置。
async fn seed_tunnel_rule(app: &common::TestApp, user_id: i64, tunnel_id: i64, port: i64) -> i64 {
    sqlx::query("INSERT OR IGNORE INTO nodes (id, name, agent_token_hash) VALUES (1, 'swn', 'x')")
        .execute(&app.state.pool)
        .await
        .unwrap();
    let res = sqlx::query(
        "INSERT INTO forward_rules (user_id, node_id, name, protocol, listen_ip, listen_port, target_host, target_port, tunnel_id) \
         VALUES (?, 1, 'tr', 'tcp', '0.0.0.0', ?, '1.2.3.4', 443, ?)",
    )
    .bind(user_id)
    .bind(port)
    .bind(tunnel_id)
    .execute(&app.state.pool)
    .await
    .unwrap();
    res.last_insert_rowid()
}

async fn cached_usage(app: &common::TestApp, uid: i64) -> i64 {
    sqlx::query_scalar("SELECT period_used_bytes_cached FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn quota_billing_applies_tunnel_ratio() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "ratiou", "password123").await.unwrap();
    // 隧道倍率 2.0、双向计费。
    let tid = sqlx::query(
        "INSERT INTO tunnels (name, transport, traffic_ratio, billing_mode) VALUES ('rt','tcp',2.0,2)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let r = seed_tunnel_rule(&app, uid, tid, 22001).await;
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) VALUES (?, datetime('now'), 100, 50)",
    )
    .bind(r)
    .execute(&app.state.pool)
    .await
    .unwrap();

    quota_tick_once(&app.state).await.unwrap();
    // (rx+tx)=150 * 2.0 = 300。
    assert_eq!(cached_usage(&app, uid).await, 300);
}

#[tokio::test]
async fn quota_billing_single_direction_counts_larger_leg() {
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "oneway", "password123").await.unwrap();
    // 单向计费(mode=1),倍率 1.0。
    let tid = sqlx::query(
        "INSERT INTO tunnels (name, transport, traffic_ratio, billing_mode) VALUES ('ow','tcp',1.0,1)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let r = seed_tunnel_rule(&app, uid, tid, 22002).await;
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) VALUES (?, datetime('now'), 100, 30)",
    )
    .bind(r)
    .execute(&app.state.pool)
    .await
    .unwrap();

    quota_tick_once(&app.state).await.unwrap();
    // 单向取较大方向 max(100,30)=100 * 1.0 = 100。
    assert_eq!(cached_usage(&app, uid).await, 100);
}

#[tokio::test]
async fn quota_billing_non_tunnel_rule_unchanged() {
    // 非隧道规则不受倍率影响,仍是 rx+tx(回归保护:默认行为不变)。
    let app = common::make_app().await.unwrap();
    let (uid, _) = common::make_user_token(&app, "plainu", "password123").await.unwrap();
    let (r1, _) = seed_rules(&app, uid).await;
    sqlx::query(
        "INSERT INTO rule_stats (rule_id, bucket_at, rx_bytes, tx_bytes) VALUES (?, datetime('now'), 100, 50)",
    )
    .bind(r1)
    .execute(&app.state.pool)
    .await
    .unwrap();

    quota_tick_once(&app.state).await.unwrap();
    assert_eq!(cached_usage(&app, uid).await, 150);
}
