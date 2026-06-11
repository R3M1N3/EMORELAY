//! P3c 隧道端到端:真 panel-server(REST + gRPC plaintext dev 模式) + 真 node-agent
//! (in-process run_agent) + 真 TCP 流量。每个测试用独立端口段防并行互撞。
mod common;

use axum::http::{Method, StatusCode};
use common::{auth_req, make_app, send, TestApp};
use emorelay_common::control::v1::control_plane_server::ControlPlaneServer;
use panel_server::grpc::service::ControlPlaneImpl;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tonic::transport::Server;

/// 起 in-process gRPC 控制面(plaintext;tests/common 的 Config dev_disable_mtls=true)。
/// 返回 (绑定地址, JoinHandle) 供测试尾 abort。
async fn start_grpc(app: &TestApp) -> (SocketAddr, JoinHandle<()>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let svc = ControlPlaneServer::new(ControlPlaneImpl::new(app.state.clone()));
    let handle = tokio::spawn(async move {
        let _ = Server::builder().add_service(svc).serve(addr).await;
    });
    // 等 server 可连。
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return (addr, handle);
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

/// 矩阵驱动:n_hops 个节点 × transport,验证 TCP 字节往返。
/// port_base:节点 i 的 pool = [port_base + i*100, port_base + i*100 + 99],
/// entry listen = port_base + 900。
async fn run_tcp_tunnel_matrix(n_hops: usize, transport: &str, port_base: u16) {
    let app = make_app().await.unwrap();
    let (grpc, grpc_handle) = start_grpc(&app).await;

    let mut node_ids = Vec::new();
    let mut agent_handles = Vec::new();
    // dirs 必须持有到测试结束,tempdir drop 会删目录。
    let mut dirs = Vec::new();
    for i in 0..n_hops {
        let lo = port_base + (i as u16) * 100;
        let (nid, token) = create_node(
            &app,
            &format!("e2e-{transport}-{n_hops}h-{i}"),
            (lo as i64, (lo + 99) as i64),
        )
        .await;
        let dir = tempfile::TempDir::new().unwrap();
        agent_handles.push(spawn_agent(grpc, nid, token, &dir));
        dirs.push(dir);
        node_ids.push(nid);
    }
    for nid in &node_ids {
        wait_node_online(&app, *nid).await;
    }

    let echo = start_tcp_echo().await;
    let tid =
        create_tunnel(&app, &format!("e2e-{transport}-{n_hops}hop"), transport, &node_ids).await;
    let listen = port_base + 900;
    create_tunnel_rule(&app, node_ids[0], tid, "tcp", listen, echo).await;

    // 用 60s 超时包裹整个连接+写读重试段,给 CI 慢机器明确失败上限。
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let payload = format!("ping-{transport}-{n_hops}");
        let payload_bytes = payload.as_bytes().to_vec();
        let mut passed = false;
        // 写读尝试多次:entry listener 就绪 ≠ exit listener / 凭据就绪(下一跳异步 apply)。
        for _ in 0..30 {
            let mut s = connect_entry(listen).await;
            // set_nodelay 在 connect 之后、首次 write 之前。
            s.set_nodelay(true).ok();
            let _ = s.write_all(&payload_bytes).await;
            let mut buf = vec![0u8; payload_bytes.len()];
            match tokio::time::timeout(Duration::from_millis(500), s.read_exact(&mut buf)).await {
                Ok(Ok(_)) if buf == payload_bytes => {
                    passed = true;
                    break;
                }
                _ => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
        passed
    })
    .await
    .expect("60s 超时:矩阵驱动未在限时内完成");

    assert!(
        result,
        "{n_hops}-hop {transport} 隧道必须把字节原样送达 echo 并返回"
    );

    for h in agent_handles {
        h.abort();
    }
    grpc_handle.abort();
}

/// 双跳 TCP 隧道端到端(回归)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hop_tcp_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(2, "tcp", 21000).await;
}

/// 三跳 TCP 隧道端到端。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hop_tcp_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(3, "tcp", 22000).await;
}

/// 双跳 TLS 隧道端到端(凭据自动下发 → apply → TlsTransport)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hop_tls_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(2, "tls", 23000).await;
}

/// 三跳 TLS 隧道端到端。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_hop_tls_tunnel_end_to_end() {
    run_tcp_tunnel_matrix(3, "tls", 24000).await;
}

/// 简单 UDP echo 目标服务。
async fn start_udp_echo() -> SocketAddr {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            let Ok((n, peer)) = sock.recv_from(&mut buf).await else {
                break;
            };
            let _ = sock.send_to(&buf[..n], peer).await;
        }
    });
    addr
}

/// 双跳 UDP-over-tunnel:UDP 包在 entry 打 2 字节长度前缀帧,经隧道流送 exit 拆帧
/// 转发 target,回程同理(P3b frame.rs 协议的全链路实测)。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hop_udp_over_tunnel_end_to_end() {
    let app = make_app().await.unwrap();
    let (grpc, grpc_handle) = start_grpc(&app).await;

    let (n1, t1) = create_node(&app, "e2e-udp-0", (25000, 25099)).await;
    let (n2, t2) = create_node(&app, "e2e-udp-1", (25100, 25199)).await;
    let d1 = tempfile::TempDir::new().unwrap();
    let d2 = tempfile::TempDir::new().unwrap();
    let a1 = spawn_agent(grpc, n1, t1, &d1);
    let a2 = spawn_agent(grpc, n2, t2, &d2);
    wait_node_online(&app, n1).await;
    wait_node_online(&app, n2).await;

    let echo = start_udp_echo().await;
    let tid = create_tunnel(&app, "e2e-2hop-udp", "tcp", &[n1, n2]).await;
    create_tunnel_rule(&app, n1, tid, "udp", 25900, echo).await;

    // UDP 无连接,入口就绪不可探测:发包 + 限时等回包,失败重发(本地回环丢包率≈0,
    // 重试覆盖的是 agent apply 异步延迟)。
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(("127.0.0.1", 25900)).await.unwrap();
    let mut buf = [0u8; 16];
    let mut got = None;
    for _ in 0..50 {
        let _ = client.send(b"udp-ping").await;
        match tokio::time::timeout(Duration::from_millis(200), client.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                got = Some(buf[..n].to_vec());
                break;
            }
            _ => continue,
        }
    }
    assert_eq!(
        got.as_deref(),
        Some(b"udp-ping".as_slice()),
        "UDP 必须经隧道帧封装往返"
    );

    a1.abort();
    a2.abort();
    grpc_handle.abort();
}
