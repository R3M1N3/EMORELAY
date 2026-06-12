// 隧道凭据轮换 sweeper:超阈值的活跃 tls/wss 隧道被重签下发(creds_rotated_at 刷新 +
// audit tunnel.creds_rotated);tcp 隧道与新近轮换过的隧道不动。
mod common;

use panel_server::models::tunnel::Tunnel;
use panel_server::sweeper::tunnel_creds::rotate_tick_once;

async fn seed_node(app: &common::TestApp, name: &str) -> i64 {
    sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip) VALUES (?, 'x', 'online', '9.9.9.9')",
    )
    .bind(name)
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn rotated_at(app: &common::TestApp, tid: i64) -> Option<String> {
    sqlx::query_scalar("SELECT creds_rotated_at FROM tunnels WHERE id = ?")
        .bind(tid)
        .fetch_one(&app.state.pool)
        .await
        .unwrap()
}

async fn set_rotated_at(app: &common::TestApp, tid: i64, modifier: &str) {
    sqlx::query("UPDATE tunnels SET creds_rotated_at = datetime('now', ?) WHERE id = ?")
        .bind(modifier)
        .bind(tid)
        .execute(&app.state.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn rotates_overdue_tls_tunnel_and_skips_fresh_and_tcp() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_node(&app, "rc-a").await;
    let n2 = seed_node(&app, "rc-b").await;

    let tls_old = Tunnel::create_with_hops(&app.state.pool, "tls-old", "tls", &[(0, n1, None), (1, n2, Some(30001))])
        .await
        .unwrap();
    let tls_fresh = Tunnel::create_with_hops(&app.state.pool, "tls-fresh", "tls", &[(0, n1, None), (1, n2, Some(30002))])
        .await
        .unwrap();
    let tcp_old = Tunnel::create_with_hops(&app.state.pool, "tcp-old", "tcp", &[(0, n1, None), (1, n2, Some(30003))])
        .await
        .unwrap();

    // 模型层直建不触发凭据下发,creds_rotated_at 为 NULL → 回落 created_at。
    // 把 tls_old / tcp_old 的轮换时间推到 25 天前(> 默认 20 天阈值),tls_fresh 设为刚轮换。
    set_rotated_at(&app, tls_old, "-25 days").await;
    set_rotated_at(&app, tcp_old, "-25 days").await;
    set_rotated_at(&app, tls_fresh, "-1 hours").await;

    let rotated = rotate_tick_once(&app.state).await.unwrap();
    assert_eq!(rotated, 1, "只有超期 tls 隧道被轮换");

    // tls_old 刷新到刚刚;tls_fresh / tcp_old 不动。
    let t_old = rotated_at(&app, tls_old).await.unwrap();
    let recent: i64 = sqlx::query_scalar(
        "SELECT CAST(strftime('%s','now') AS INTEGER) - CAST(strftime('%s', ?) AS INTEGER)",
    )
    .bind(&t_old)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(recent < 60, "creds_rotated_at 应刷新为当前时间,实际 {recent}s 前");

    // tcp 隧道的时间戳保持 25 天前的旧值(未被轮换触碰)。
    let t_tcp = rotated_at(&app, tcp_old).await.unwrap();
    let tcp_age: i64 = sqlx::query_scalar(
        "SELECT CAST(strftime('%s','now') AS INTEGER) - CAST(strftime('%s', ?) AS INTEGER)",
    )
    .bind(&t_tcp)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert!(tcp_age > 24 * 86400, "tcp 隧道不参与轮换,时间戳不应被刷新");

    // audit 落了系统轮换记录(无 actor)。
    let (count, actor): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(actor_user_id) FROM audit_logs \
         WHERE action = 'tunnel.creds_rotated' AND target_id = ?",
    )
    .bind(tls_old)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "应有一条轮换 audit");
    assert!(actor.is_none(), "系统动作无 actor");

    // 再跑一轮:已刷新,不重复轮换。
    let rotated2 = rotate_tick_once(&app.state).await.unwrap();
    assert_eq!(rotated2, 0, "刚轮换过的隧道下个 tick 不应重复轮换");
}

#[tokio::test]
async fn null_rotated_at_falls_back_to_created_at() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_node(&app, "nf-a").await;
    let n2 = seed_node(&app, "nf-b").await;
    let tid = Tunnel::create_with_hops(&app.state.pool, "nf-tls", "tls", &[(0, n1, None), (1, n2, Some(30011))])
        .await
        .unwrap();
    // creds_rotated_at 保持 NULL,把 created_at 推老 → 回落判定应命中。
    sqlx::query("UPDATE tunnels SET created_at = datetime('now', '-25 days') WHERE id = ?")
        .bind(tid)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert!(rotated_at(&app, tid).await.is_none(), "前置:rotated_at 为 NULL");

    let rotated = rotate_tick_once(&app.state).await.unwrap();
    assert_eq!(rotated, 1, "NULL rotated_at 按 created_at 回落参与轮换");
    assert!(rotated_at(&app, tid).await.is_some(), "轮换后时间戳写入");
}

// 软删隧道不轮换(query 已过滤,防御性断言)。
#[tokio::test]
async fn soft_deleted_tunnel_is_ignored() {
    let app = common::make_app().await.unwrap();
    let n1 = seed_node(&app, "sd-a").await;
    let n2 = seed_node(&app, "sd-b").await;
    let tid = Tunnel::create_with_hops(&app.state.pool, "sd-tls", "tls", &[(0, n1, None), (1, n2, Some(30021))])
        .await
        .unwrap();
    set_rotated_at(&app, tid, "-25 days").await;
    Tunnel::soft_delete(&app.state.pool, tid).await.unwrap();

    let rotated = rotate_tick_once(&app.state).await.unwrap();
    assert_eq!(rotated, 0);
}
