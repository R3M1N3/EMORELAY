//! Task 1(稳健性加固):diagnose 端点 per-user 限流 + probe_waiters 全局上限。
//! 任意认证用户高频调用 diagnose 会无界注册 probe_waiters(隧道按跳数 fan-out 放大),
//! 构成 DoS。这里验证:① 同一 user 超过 burst 后被限流 429;② probe_waiters 达上限
//! 后新诊断被拒 429;③ 正常单次诊断不受影响;④ 多段(隧道)诊断命中上限返回 429 且
//! 不泄漏已注册的兄弟段等待者(run_segments 不得 abort 在途任务)。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send};
use serde_json::json;
use tokio::sync::oneshot;

/// 建 N 个 online 节点(带 public_ip + port_pool),返回 ids。
async fn seed_online_nodes(app: &common::TestApp, n: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = sqlx::query(
            "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
             VALUES (?, 'x', 'online', ?, 30000, 30010)",
        )
        .bind(format!("dn{i}"))
        .bind(format!("10.2.0.{i}"))
        .execute(&app.state.pool)
        .await
        .unwrap()
        .last_insert_rowid();
        ids.push(id);
    }
    ids
}

/// 建一个在线节点 + 一条非隧道规则,返回 (node_id, rule_id)。
async fn make_node_and_rule(app: &common::TestApp) -> (i64, i64) {
    let node_id = sqlx::query(
        "INSERT INTO nodes (name, agent_token_hash, status, public_ip, port_pool_min, port_pool_max) \
         VALUES ('dn','x','online','1.2.3.4',10000,65535)",
    )
    .execute(&app.state.pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": node_id, "name": "r", "protocol": "tcp", "listen_port": 20000,
            "target_host": "9.9.9.9", "target_port": 443
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create rule: {body}");
    (node_id, body["id"].as_i64().unwrap())
}

/// 同一 user 在限流窗口内连续调 diagnose,超过 burst(3)后第 4 次起返回 429。
/// 节点离线(无 agent 订阅),每次 diagnose 快速返回 dispatched:false,不依赖 agent。
#[tokio::test]
async fn diagnose_rate_limited_per_user() {
    let app = make_app().await.unwrap();
    let (_node, rule_id) = make_node_and_rule(&app).await;

    let mut statuses = Vec::new();
    for _ in 0..4 {
        let req = auth_req(
            Method::POST,
            &format!("/api/rules/{rule_id}/diagnose"),
            &app.admin_token,
            None,
        )
        .unwrap();
        let (status, _body) = send(app.app.clone(), req).await.unwrap();
        statuses.push(status.as_u16());
    }
    // burst 3:前 3 次放行(200),第 4 次被限流(429)。
    assert_eq!(
        statuses,
        vec![200, 200, 200, 429],
        "同一 user 超过 burst 3 后应 429"
    );
}

/// probe_waiters 达到全局上限(64)后,新 diagnose 的 register_probe 被拒,
/// handler 映射为 HTTP 429「诊断繁忙」。这里直接占满 64 个等待者再发起一次诊断。
#[tokio::test]
async fn probe_waiters_capacity_rejects() {
    let app = make_app().await.unwrap();
    let (_node, rule_id) = make_node_and_rule(&app).await;

    // 占满 probe_waiters 到上限 64(持有 receiver 防止 oneshot 关闭;sender 留在 map 中
    // 计入 len)。map 仅由 resolve_probe / cancel_probe 排空,故记下 id 供后续释放。
    let mut held_ids: Vec<String> = Vec::new();
    let mut held_rx: Vec<oneshot::Receiver<_>> = Vec::new();
    for _ in 0..64 {
        let (id, rx) = app
            .state
            .register_probe()
            .expect("前 64 个 register 应成功");
        held_ids.push(id);
        held_rx.push(rx);
    }

    // 第 65 个诊断:run_one 内首个 register_probe 即命中上限 → 整请求 429。
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/diagnose"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "probe_waiters 满后应 429: {body}"
    );

    // 经 cancel_probe 排空一个等待者后,容量恢复、register 又能成功(上限是动态的、非一次性闸)。
    app.state.cancel_probe(&held_ids[0]);
    assert!(
        app.state.register_probe().is_ok(),
        "释放一个槽位后容量应恢复"
    );
    drop(held_rx);
}

/// 多段(隧道)诊断命中 probe_waiters 上限:整请求 429,且**不泄漏**已成功注册的兄弟段
/// 等待者。run_segments 早返时若 drop JoinSet 会 abort 仍在 await 的兄弟任务,使其错过
/// cancel_probe 造成泄漏(map 无 sweeper)。修复后 run_segments 跑完所有任务再传播错误,
/// 故已注册的兄弟段被解析后能完成自身清理。
///
/// 构造确定性场景:上限只剩 1 槽 + 节点已订阅(dispatch 成功 → 探测 park 在超时 await)。
/// 2 段中恰好 1 段抢到槽位并 park,另 1 段命中上限返回 Err。测试扮演 Agent 解析 park 的探测,
/// 使修复版 drain 快速完成并返回 429;随后 map 不应泄漏。
#[tokio::test]
async fn tunnel_diagnose_capacity_does_not_leak_siblings() {
    let app = make_app().await.unwrap();
    let nodes = seed_online_nodes(&app, 2).await;
    // 两节点都订阅 mock agent → 段 dispatch 成功并 park。
    let mut rxs: Vec<_> = nodes
        .iter()
        .map(|n| app.state.dispatcher.subscribe(*n).0)
        .collect();
    // 2 跳隧道 + 其上一条规则 → 诊断产出 2 段(hop0→hop1、出口→目标)。
    let tid = {
        let req = auth_req(
            Method::POST,
            "/api/tunnels",
            &app.admin_token,
            Some(json!({ "name": "lt", "transport": "tcp", "node_ids": nodes })),
        )
        .unwrap();
        let (status, body) = send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::OK, "create tunnel: {body}");
        body["id"].as_i64().unwrap()
    };
    let rule_id = {
        let req = auth_req(
            Method::POST,
            "/api/rules",
            &app.admin_token,
            Some(json!({
                "node_id": nodes[0], "name": "r", "protocol": "tcp", "listen_port": 20000,
                "target_host": "9.9.9.9", "target_port": 443, "tunnel_id": tid
            })),
        )
        .unwrap();
        let (status, body) = send(app.app.clone(), req).await.unwrap();
        assert_eq!(status, StatusCode::OK, "create rule: {body}");
        body["id"].as_i64().unwrap()
    };
    // 隧道/规则创建已向两节点下发 ApplyRule 等命令;清空通道,只关心诊断阶段的 Probe。
    for rx in rxs.iter_mut() {
        while rx.try_recv().is_ok() {}
    }

    // 占满到只剩 1 槽(63):2 段中恰好 1 段能注册并 park,另 1 段命中上限。
    let mut held_rx: Vec<oneshot::Receiver<_>> = Vec::new();
    for _ in 0..63 {
        let (_id, rx) = app.state.register_probe().expect("前 63 个 register 应成功");
        held_rx.push(rx);
    }

    // 后台发起诊断(修复版会 drain,阻塞直到 park 的探测被解析/超时)。
    let app_clone = app.app.clone();
    let token = app.admin_token.clone();
    let diag = tokio::spawn(async move {
        let req = auth_req(
            Method::POST,
            &format!("/api/rules/{rule_id}/diagnose"),
            &token,
            None,
        )
        .unwrap();
        send(app_clone, req).await.unwrap()
    });

    // 先确认那条段确实 park 了(dispatch 成功、命令进了通道),再校验请求仍在途。
    let state = app.state.clone();
    let mut parked_probe_id: Option<String> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && parked_probe_id.is_none() {
        for rx in rxs.iter_mut() {
            if let Ok(cmd) = rx.try_recv() {
                if let Some(emorelay_common::control::v1::command::Body::Probe(p)) = cmd.body {
                    parked_probe_id = Some(p.probe_id);
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let parked_probe_id = parked_probe_id.expect("应有 1 段成功 dispatch 并 park");

    // 关键判别(无泄漏 / 不 abort):此刻 park 的探测尚未解析,修复版必仍在 drain 等待,
    // 请求不应已返回。旧 bug(`?` 早返 + abort 兄弟)会立刻返回 429,这里即 is_finished。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(
        !diag.is_finished(),
        "请求过早返回:run_segments 提前 abort 了在途兄弟段(旧泄漏 bug)"
    );

    // 扮演 Agent 解析 park 的探测 → 放行 drain → 请求返回 429。
    state.resolve_probe(emorelay_common::control::v1::ProbeResult {
        probe_id: parked_probe_id,
        reachable: true,
        avg_latency_ms: 1.0,
        loss_pct: 0.0,
        error: String::new(),
    });
    let (status, body) = diag.await.unwrap();
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "多段诊断命中上限应整请求 429: {body}"
    );

    // 兜底:请求结束后 map 应留有空槽(已注册兄弟段被正常清理,无残留)→ register 必成功。
    assert!(
        app.state.register_probe().is_ok(),
        "命中上限后 map 仍满,疑似等待者泄漏"
    );

    drop(held_rx);
}

/// 正常单次诊断不受限流 / 上限影响:离线节点返回 200 且 dispatched:false。
#[tokio::test]
async fn diagnose_single_ok() {
    let app = make_app().await.unwrap();
    let (_node, rule_id) = make_node_and_rule(&app).await;
    let req = auth_req(
        Method::POST,
        &format!("/api/rules/{rule_id}/diagnose"),
        &app.admin_token,
        None,
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["segments"][0]["dispatched"], false);
}
