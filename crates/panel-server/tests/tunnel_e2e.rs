//! P3c 隧道端到端:真 panel-server(REST + gRPC plaintext dev 模式) + 真 node-agent
//! (in-process run_agent) + 真 TCP 流量。每个测试用独立端口段防并行互撞。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send, TestApp};
use emorelay_common::control::v1::{
    control_plane_server::ControlPlaneServer,
};
use panel_server::grpc::service::ControlPlaneImpl;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tonic::transport::Server;

/// 起 in-process gRPC 控制面(plaintext;tests/common 的 Config dev_disable_mtls=true)。
async fn start_grpc(app: &TestApp) -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let svc = ControlPlaneServer::new(ControlPlaneImpl::new(app.state.clone()));
    tokio::spawn(async move {
        let _ = Server::builder().add_service(svc).serve(addr).await;
    });
    // 等 server 可连。
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("grpc server not up at {addr}");
}

/// REST 创建节点(public_ip=127.0.0.1 供下游 hop dial),返回 (node_id, agent_token)。
async fn create_node(app: &TestApp, name: &str, pool: (i64, i64)) -> (i64, String) {
    let req = auth_req(
        Method::POST,
        "/api/nodes",
        &app.admin_token,
        Some(json!({
            "name": name,
            "public_ip": "127.0.0.1",
            "grpc_endpoint": "127.0.0.1:0",
            "port_pool_min": pool.0,
            "port_pool_max": pool.1,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create_node: {body}");
    (
        body["node"]["id"].as_i64().unwrap(),
        body["agent_token"].as_str().unwrap().to_string(),
    )
}

/// in-process 起真 agent(plaintext 控制面)。返回 handle 供测试尾 abort。
fn spawn_agent(
    grpc: SocketAddr,
    node_id: i64,
    token: String,
    dir: &tempfile::TempDir,
) -> tokio::task::JoinHandle<()> {
    let base = dir.path().display().to_string().replace('\\', "/");
    let cfg = node_agent::config::Config {
        node_id,
        control_endpoint: format!("http://{grpc}"),
        token,
        state_path: format!("{base}/agent-state.json"),
        data_dir: base,
        grpc_ca_cert: None,
        grpc_client_cert: None,
        grpc_client_key: None,
    };
    tokio::spawn(async move {
        let _ = node_agent::run_agent(cfg).await;
    })
}

async fn wait_node_online(app: &TestApp, node_id: i64) {
    for _ in 0..100 {
        let (status,): (String,) = sqlx::query_as("SELECT status FROM nodes WHERE id = ?")
            .bind(node_id)
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        if status == "online" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node {node_id} never came online");
}

/// 简单 TCP echo 目标服务。
async fn start_tcp_echo() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// 入口端口可达前轮询重连(agent apply 是异步下发)。
async fn connect_entry(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)).await {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("entry port {port} never accepted");
}

/// REST 建隧道 → 返回 tunnel_id。
async fn create_tunnel(app: &TestApp, name: &str, transport: &str, node_ids: &[i64]) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/tunnels",
        &app.admin_token,
        Some(json!({
            "name": name,
            "transport": transport,
            "node_ids": node_ids,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create_tunnel: {body}");
    body["id"].as_i64().unwrap()
}

/// REST 建隧道入口规则(listen_ip=127.0.0.1)。
async fn create_tunnel_rule(
    app: &TestApp,
    entry_node: i64,
    tunnel_id: i64,
    protocol: &str,
    listen_port: u16,
    target: SocketAddr,
) -> i64 {
    let req = auth_req(
        Method::POST,
        "/api/rules",
        &app.admin_token,
        Some(json!({
            "node_id": entry_node,
            "name": format!("e2e-{protocol}-{listen_port}"),
            "protocol": protocol,
            "listen_ip": "127.0.0.1",
            "listen_port": listen_port,
            "target_host": "127.0.0.1",
            "target_port": target.port(),
            "tunnel_id": tunnel_id,
        })),
    )
    .unwrap();
    let (status, body) = send(app.app.clone(), req).await.unwrap();
    assert_eq!(status, StatusCode::OK, "create_tunnel_rule: {body}");
    body["id"].as_i64().unwrap()
}

/// 双跳 TCP:client → entry(n1, 21500) → exit(n2, inter_port) → echo。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hop_tcp_tunnel_end_to_end() {
    let app = make_app().await.unwrap();
    let grpc = start_grpc(&app).await;

    let (n1, t1) = create_node(&app, "e2e-hop-0", (21000, 21099)).await;
    let (n2, t2) = create_node(&app, "e2e-hop-1", (21100, 21199)).await;

    let d1 = tempfile::TempDir::new().unwrap();
    let d2 = tempfile::TempDir::new().unwrap();
    let a1 = spawn_agent(grpc, n1, t1, &d1);
    let a2 = spawn_agent(grpc, n2, t2, &d2);
    wait_node_online(&app, n1).await;
    wait_node_online(&app, n2).await;

    let echo = start_tcp_echo().await;
    let tid = create_tunnel(&app, "e2e-2hop-tcp", "tcp", &[n1, n2]).await;
    create_tunnel_rule(&app, n1, tid, "tcp", 21500, echo).await;

    // 写读尝试多次:entry listener 就绪 ≠ exit listener 就绪(下一跳 Rule 可能稍后 apply)。
    let mut passed = false;
    for _ in 0..30 {
        let mut s = connect_entry(21500).await;
        let _ = s.write_all(b"hello-tunnel").await;
        s.set_nodelay(true).ok();
        // 设置读超时避免 entry 尚未转发时永久阻塞。
        let mut buf = [0u8; 12];
        match tokio::time::timeout(Duration::from_millis(500), s.read_exact(&mut buf)).await {
            Ok(Ok(_)) if &buf == b"hello-tunnel" => {
                passed = true;
                break;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
    assert!(passed, "双跳 TCP 隧道必须把字节原样送达 echo 并返回");

    a1.abort();
    a2.abort();
}
