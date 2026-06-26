mod common;

use axum::http::Method;
use serde_json::json;
use std::time::Duration;

/// 建一个 online 节点(带 public_ip + port_pool),返回 id。
async fn seed_online_node(app: &common::TestApp) -> i64 {
    sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES ('rn', 'x', 'online', '10.9.0.1', 30000, 30010)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

/// 在某节点上建一条非隧道规则,返回 rule_id。
async fn create_rule(app: &common::TestApp, node_id: i64, port: u16) -> i64 {
    let req = common::auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "r", "protocol": "tcp",
            "listen_port": port, "target_host": "9.9.9.9", "target_port": 443
        })),
    )
    .unwrap();
    let (status, body) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    body["id"].as_i64().unwrap()
}

/// 核心回归(Task 3):reconcile「快照读 + 下发」与 delete「下发」对同一 node 持同一把
/// per-node 锁。本测试持有该 node 锁,模拟一次正在进行中的 reconcile(持锁期间),
/// 断言并发 delete 在锁释放前**无法完成**(被串行化阻塞);锁释放后 delete 立即完成。
/// 这正是关闭「delete 后被 reconcile 按旧快照复活」窗口的串行保障。
/// 锁 API 不存在(当前代码)时本测试编译失败 = RED。
#[tokio::test]
async fn concurrent_delete_blocks_while_node_lock_held() {
    let app = common::make_app().await.unwrap();
    let node = seed_online_node(&app).await;
    // 模拟 Agent 在线(dispatcher 有该 node channel),持有 rx 防止下发即丢。
    let _rx = app.state.dispatcher.subscribe(node).0;
    let rule_id = create_rule(&app, node, 30000).await;

    // 持有该 node 的 per-node dispatch 锁(模拟 reconcile 快照读+下发进行中)。
    let guards = app.state.dispatcher.lock_nodes(&[node]).await;

    // 并发发起 delete:它必须先抢同一把 node 锁,故应阻塞在锁上。
    let app_clone = app.app.clone();
    let token = app.admin_token.clone();
    let mut delete_task = tokio::spawn(async move {
        let req = common::auth_req(
            Method::DELETE,
            &format!("/api/rules/{rule_id}"),
            &token,
            None,
        )
        .unwrap();
        common::send(app_clone, req).await.unwrap()
    });

    // 锁未释放期间,delete 不应完成(被 per-node 锁串行化阻塞)。
    // `&mut JoinHandle` 是 Future;短超时探测它仍 pending。
    let pending = tokio::time::timeout(Duration::from_millis(300), &mut delete_task).await;
    assert!(
        pending.is_err(),
        "持有 node 锁期间 delete 不应完成(应被 per-node 串行锁阻塞)"
    );

    // 释放锁后,delete 应迅速完成并成功。
    drop(guards);
    let (status, _body) = tokio::time::timeout(Duration::from_secs(5), &mut delete_task)
        .await
        .expect("释放锁后 delete 应迅速完成")
        .expect("delete 任务不应 panic");
    assert_eq!(status, axum::http::StatusCode::OK);
}

/// 核心回归(Bug #1,对称 delete 测试):create 的 `dispatch_rule_apply` 也必须被同一把
/// per-node 锁覆盖,否则并发 create 的 ApplyRule 会夹在 reconcile 的「快照读」与末尾权威
/// `ReconcileRules`(旧快照不含新规则)之间 → Agent 先 apply 再被当孤儿删 → 规则 DB 存在
/// 但数据面永久死、在线不自愈。本测试持有该 node 锁(模拟 reconcile 进行中),断言并发 create
/// 在锁释放前**无法完成**(其 dispatch 被串行化阻塞);锁释放后立即完成 200。
/// create 在持锁前已完成 DB 写入与校验,阻塞点正是 dispatch 入队,精确锁住「入队↔reconcile」互斥。
#[tokio::test]
async fn concurrent_create_blocks_while_node_lock_held() {
    let app = common::make_app().await.unwrap();
    let node = seed_online_node(&app).await;
    // 模拟 Agent 在线(dispatcher 有该 node channel),持有 rx 防止下发即丢。
    let _rx = app.state.dispatcher.subscribe(node).0;

    // 持有该 node 的 per-node dispatch 锁(模拟 reconcile 快照读+下发进行中)。
    let guards = app.state.dispatcher.lock_nodes(&[node]).await;

    // 并发发起 create:它在写库/校验后须抢同一把 node 锁才能 dispatch,故应阻塞在锁上。
    let app_clone = app.app.clone();
    let token = app.admin_token.clone();
    let mut create_task = tokio::spawn(async move {
        let req = common::auth_req(
            Method::POST,
            "/api/rules",
            &token,
            Some(json!({
                "node_id": node, "name": "r", "protocol": "tcp",
                "listen_port": 30002, "target_host": "9.9.9.9", "target_port": 443
            })),
        )
        .unwrap();
        common::send(app_clone, req).await.unwrap()
    });

    // 锁未释放期间,create 不应完成(dispatch 被 per-node 锁串行化阻塞)。
    let pending = tokio::time::timeout(Duration::from_millis(300), &mut create_task).await;
    assert!(
        pending.is_err(),
        "持有 node 锁期间 create 不应完成(其 dispatch 应被 per-node 串行锁阻塞)"
    );

    // 释放锁后,create 应迅速完成并成功。
    drop(guards);
    let (status, body) = tokio::time::timeout(Duration::from_secs(5), &mut create_task)
        .await
        .expect("释放锁后 create 应迅速完成")
        .expect("create 任务不应 panic");
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
}

/// 串行化保留 reconcile 自愈语义:delete 完整完成(软删 + RemoveRule 下发)后,
/// 后续 reconcile 读取的快照已不含被删规则 → keep_ids 不会把它复活。
/// (本断言在串行化前后都成立,作为「自愈语义未被破坏」的回归保护。)
#[tokio::test]
async fn reconcile_after_delete_excludes_deleted_rule() {
    let app = common::make_app().await.unwrap();
    let node = seed_online_node(&app).await;
    let _rx = app.state.dispatcher.subscribe(node).0;
    let rule_id = create_rule(&app, node, 30001).await;

    // delete 完整走完(含 per-node 锁内的软删 + RemoveRule 下发)。
    let req = common::auth_req(Method::DELETE, &format!("/api/rules/{rule_id}"), &app.admin_token, None).unwrap();
    let (status, _) = common::send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, axum::http::StatusCode::OK);

    // 此后 reconcile 的权威 keep_ids 必不含被删规则。
    let cmds = panel_server::grpc::tunnel_dispatch::reconcile_commands_for_node(&app.state, node)
        .await
        .expect("reconcile");
    let keep = panel_server::grpc::tunnel_dispatch::authoritative_rule_ids(&cmds);
    assert!(
        !keep.contains(&rule_id),
        "被删规则不得出现在 reconcile keep_ids(否则会被复活)"
    );
}
