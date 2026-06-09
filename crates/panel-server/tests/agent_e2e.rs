// gRPC 协议级 e2e: 验证 plan §13 验收 #4 (Agent 连接主控并显示在线) 与 #8
// (前端能看到规则流量统计) 的服务器端协议链路。模拟 Agent 用 tonic client
// 跑通 register → subscribe → 接收 ApplyRule → report_rule_stats → DB 累计的
// 完整环路。实际 TCP relay 由 node-agent::relay 的 unit test 覆盖,本测试不重复。

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use chrono::Utc;
use common::{auth_req, make_app, send};
use emorelay_common::control::v1::{
    command::Body, control_plane_client::ControlPlaneClient,
    control_plane_server::ControlPlaneServer, RegisterRequest, RuleStatsBatch, RuleStatsBucket,
    SubscribeRequest,
};
use panel_server::grpc::{service::ControlPlaneImpl, SESSION_METADATA_KEY};
use serde_json::json;
use tokio_stream::StreamExt;
use tonic::transport::{Channel, Server};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_e2e_register_dispatch_stats() {
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    // 1) 起 gRPC server 在随机端口 (bind 0 → drop → 重 bind, localhost race 可忽略)
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let svc_state = app.state.clone();
    let server_handle = tokio::spawn(async move {
        let svc = ControlPlaneServer::new(ControlPlaneImpl::new(svc_state));
        Server::builder().add_service(svc).serve(addr).await
    });
    // 让 server 完成 bind;localhost 上 ~50ms 足够
    tokio::time::sleep(Duration::from_millis(250)).await;

    // 2) REST 建 node 拿 agent_token (创建路径返回明文 token, DB 只存 SHA-256)
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "e2e-node" })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create node: {body}");
    let node_id = body["node"]["id"].as_i64().unwrap();
    let agent_token = body["agent_token"].as_str().unwrap().to_string();

    // 3) tonic client 连
    let endpoint = format!("http://{addr}");
    let channel = Channel::from_shared(endpoint)
        .unwrap()
        .connect()
        .await
        .expect("tonic connect");
    let mut client = ControlPlaneClient::new(channel);

    // 4) Register → session_token
    let resp = client
        .register(RegisterRequest {
            node_id,
            agent_token: agent_token.clone(),
            version: "e2e-test/0.1".into(),
        })
        .await
        .expect("register")
        .into_inner();
    let session_token = resp.session_token;
    assert!(!session_token.is_empty(), "session_token 必须非空");
    assert!(
        resp.expires_at_unix > Utc::now().timestamp(),
        "session 必须未过期"
    );

    // 5) 验证 node.status='online' (plan §13 #4)
    let status_row: (String,) = sqlx::query_as("SELECT status FROM nodes WHERE id = ?")
        .bind(node_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(status_row.0, "online", "register 必须把 node 标 online");

    // 6) SubscribeCommands 开 stream (附带 session metadata)
    let mut sub_req = tonic::Request::new(SubscribeRequest { node_id });
    sub_req.metadata_mut().insert(
        SESSION_METADATA_KEY,
        tonic::metadata::MetadataValue::try_from(&session_token).unwrap(),
    );
    let mut stream = client
        .subscribe_commands(sub_req)
        .await
        .expect("subscribe_commands")
        .into_inner();
    // 让 server reconcile 与 channel install 完成 (fresh DB reconciled=0)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 7) REST 创建 rule → server 端 dispatcher 同步 push 到 SubscribeCommands stream
    let req = auth_req(
        Method::POST,
        "/api/rules",
        t,
        Some(json!({
            "node_id": node_id,
            "name": "e2e-rule",
            "protocol": "tcp",
            "listen_port": 20000,
            "target_host": "1.2.3.4",
            "target_port": 80,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create rule: {body}");
    let rule_id = body["id"].as_i64().unwrap();

    // 8) Agent (mock) 收到 ApplyRule
    let cmd = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("subscribe 等命令超时")
        .expect("stream 提前结束")
        .expect("stream 错误");
    match cmd.body {
        Some(Body::ApplyRule(apply)) => {
            let r = apply.rule.expect("ApplyRule.rule 缺失");
            assert_eq!(r.id, rule_id);
            assert_eq!(r.protocol, "tcp");
            assert_eq!(r.listen_port, 20000);
            assert_eq!(r.target_host, "1.2.3.4");
            assert_eq!(r.target_port, 80);
            assert!(r.enabled, "新建规则默认 enabled");
        }
        other => panic!("expected ApplyRule, got {other:?}"),
    }

    // 9) Agent (mock) 上报 rule stats (client-streaming)
    let now_unix = Utc::now().timestamp();
    let batch = RuleStatsBatch {
        node_id,
        buckets: vec![RuleStatsBucket {
            rule_id,
            bucket_at_unix: now_unix,
            rx_bytes: 100,
            tx_bytes: 200,
            connection_count: 1,
            error_count: 0,
        }],
    };
    let stream_in = tokio_stream::iter(vec![batch]);
    let mut report_req = tonic::Request::new(stream_in);
    report_req.metadata_mut().insert(
        SESSION_METADATA_KEY,
        tonic::metadata::MetadataValue::try_from(&session_token).unwrap(),
    );
    let ack = client
        .report_rule_stats(report_req)
        .await
        .expect("report_rule_stats")
        .into_inner();
    assert!(ack.ok, "ack 应成功: error={}", ack.error);

    // 10) 验证 forward_rules 累计 (plan §13 #8)
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT rx_bytes, tx_bytes, connection_count FROM forward_rules WHERE id = ?",
    )
    .bind(rule_id)
    .fetch_one(&app.state.pool)
    .await
    .unwrap();
    assert_eq!(row.0, 100, "rx_bytes 累计");
    assert_eq!(row.1, 200, "tx_bytes 累计");
    assert_eq!(row.2, 1, "connection_count 累计");

    // 11) 验证 rule_stats bucket 有写入
    let bucket_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rule_stats WHERE rule_id = ?")
        .bind(rule_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(bucket_count, 1);

    // 12) 防回归: 无 traffic_limit 时 report_rule_stats 不应触发 auto_stop_if_exceeded
    let enabled: i64 = sqlx::query_scalar("SELECT enabled FROM forward_rules WHERE id = ?")
        .bind(rule_id)
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(enabled, 1, "无 limit 时不应自动停规则");

    server_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_e2e_bad_token_rejected_with_same_error() {
    // plan §9 安全:防止通过差异化错误信息枚举 node_id。
    // unknown_node 与 bad_token 必须返回同样的 PermissionDenied 状态。
    let app = make_app().await.unwrap();
    let t = &app.admin_token;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let svc_state = app.state.clone();
    let server_handle = tokio::spawn(async move {
        let svc = ControlPlaneServer::new(ControlPlaneImpl::new(svc_state));
        Server::builder().add_service(svc).serve(addr).await
    });
    tokio::time::sleep(Duration::from_millis(250)).await;

    // 真的建一个 node,但用错误 token
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        t,
        Some(json!({ "name": "real-node" })),
    )
    .unwrap();
    let (_, body) = send(app.app.clone(), req).await.unwrap();
    let node_id = body["node"]["id"].as_i64().unwrap();

    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = ControlPlaneClient::new(channel);

    // 已知 node + 错 token
    let bad = client
        .register(RegisterRequest {
            node_id,
            agent_token: "wrong-token".into(),
            version: "e2e/0.1".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code(), tonic::Code::PermissionDenied);
    let bad_msg = bad.message().to_string();

    // 未知 node id
    let unknown = client
        .register(RegisterRequest {
            node_id: 999_999,
            agent_token: "anything".into(),
            version: "e2e/0.1".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), tonic::Code::PermissionDenied);
    let unknown_msg = unknown.message().to_string();

    assert_eq!(bad_msg, unknown_msg, "两种错误必须返回相同消息以防枚举");

    server_handle.abort();
}
