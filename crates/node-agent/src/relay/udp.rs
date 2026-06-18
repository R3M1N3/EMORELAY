use anyhow::{Context, Result};
use emorelay_common::control::v1::Rule;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{error, info, warn};

use crate::limit::TokenBucket;
use crate::stats::{RuleCounter, StatsCollector};

/// 客户端空闲多久后清理 NAT session。
const SESSION_TIMEOUT: Duration = Duration::from_secs(120);
/// 多久跑一次 sweep。
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// 最大单包 UDP 字节（IPv4 64KB - header）。
const MAX_PACKET: usize = 65535;
/// UDP NAT 表上限:每 session 占一个 upstream socket(fd + ephemeral 端口)与一个反向 task。
/// 源地址可伪造,无上限会被海量伪造源耗尽 fd/端口/内存;达上限丢新包(既有 session 不受影响)。
const MAX_UDP_SESSIONS: usize = 8192;

pub struct UdpRelayHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl UdpRelayHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

struct Session {
    upstream: Arc<UdpSocket>,
    last_seen: Instant,
    upstream_task: JoinHandle<()>,
}

pub async fn start(
    rule: Rule,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<UdpRelayHandle> {
    start_inner(rule, stats, SESSION_TIMEOUT, SWEEP_INTERVAL, MAX_UDP_SESSIONS, bucket).await
}

async fn start_inner(
    rule: Rule,
    stats: Arc<StatsCollector>,
    session_timeout: Duration,
    sweep_interval: Duration,
    max_sessions: usize,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<UdpRelayHandle> {
    let listen_ip: IpAddr = rule
        .listen_ip
        .parse()
        .with_context(|| format!("invalid listen_ip: {}", rule.listen_ip))?;
    let listen_port: u16 = u16::try_from(rule.listen_port)
        .with_context(|| format!("listen_port out of u16 range: {}", rule.listen_port))?;
    let addr = SocketAddr::new(listen_ip, listen_port);
    let listener = Arc::new(
        UdpSocket::bind(addr)
            .await
            .with_context(|| format!("udp bind {addr}"))?,
    );
    info!(rule_id = rule.id, %addr, "udp relay listening");

    let counter = stats.ensure(rule.id);
    let target_host = rule.target_host.clone();
    let target_port: u16 = u16::try_from(rule.target_port)
        .with_context(|| format!("target_port out of u16 range: {}", rule.target_port))?;
    let rule_id = rule.id;
    let sessions: Arc<Mutex<HashMap<SocketAddr, Session>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_PACKET];
        let mut sweep_tick = interval(sweep_interval);
        sweep_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        sweep_tick.tick().await; // 立即触发的首个 tick 消费掉

        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!(rule_id, "udp relay stopping");
                    break;
                }
                res = listener.recv_from(&mut buf) => {
                    match res {
                        Ok((n, client_addr)) => {
                            // recv 即计 tx_bytes:限速丢掉的包仍算"收到过"的流量。
                            counter.tx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                            if let Err(e) = forward(
                                rule_id,
                                &listener,
                                client_addr,
                                &buf[..n],
                                &sessions,
                                max_sessions,
                                &target_host,
                                target_port,
                                &counter,
                                &bucket,
                            ).await {
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                                warn!(rule_id, %client_addr, error = ?e, "udp forward error");
                            }
                        }
                        Err(e) => {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            error!(rule_id, error = ?e, "udp recv error");
                        }
                    }
                }
                _ = sweep_tick.tick() => {
                    sweep_expired(rule_id, &sessions, session_timeout).await;
                }
            }
        }

        // 停止时把所有 session 的反向 task abort，释放 upstream socket。
        let mut map = sessions.lock().await;
        for (_, session) in map.drain() {
            session.upstream_task.abort();
        }
    });

    Ok(UdpRelayHandle { stop_tx, join })
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    rule_id: i64,
    listener: &Arc<UdpSocket>,
    client_addr: SocketAddr,
    data: &[u8],
    sessions: &Arc<Mutex<HashMap<SocketAddr, Session>>>,
    max_sessions: usize,
    target_host: &str,
    target_port: u16,
    counter: &Arc<RuleCounter>,
    bucket: &Option<Arc<TokenBucket>>,
) -> Result<()> {
    // 限速:配额不足直接丢包(UDP 语义,不阻塞事件循环)。
    if let Some(b) = bucket {
        if !b.try_acquire(data.len()) {
            counter.error_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }
    let mut map = sessions.lock().await;
    // I3:反向 task 已死(client 不可达等)的既有 session 先移除,落到下方重建逻辑恢复回程,
    // 否则 last_seen 被持续刷新导致永不超时、回程永久中断。
    if map
        .get(&client_addr)
        .is_some_and(|s| s.upstream_task.is_finished())
    {
        map.remove(&client_addr);
    }
    if let Some(s) = map.get_mut(&client_addr) {
        s.last_seen = Instant::now();
        s.upstream
            .send(data)
            .await
            .context("upstream send (existing session)")?;
        return Ok(());
    }

    // NAT 表上限:达上限丢弃新源的包,防伪造源耗尽 fd/端口/内存(既有 session 上面已返回)。
    if map.len() >= max_sessions {
        counter.error_count.fetch_add(1, Ordering::Relaxed);
        warn!(rule_id, %client_addr, max = max_sessions, "udp sessions at cap; dropping new-session packet");
        return Ok(());
    }

    // 新 session：用临时端口的 upstream socket + connect 默认 peer。
    // 域名目标的 connect 走阻塞 getaddrinfo,无上限时慢/不可达 DNS 会把本规则唯一的
    // recv 循环连同 sweep 一起停摆(本段在主循环内 inline await)。与隧道 UDP 路径
    // (tunnel/task.rs)一致套 5s 超时;超时按错误丢包(UDP 语义,不阻塞循环)。
    let upstream = match tokio::time::timeout(Duration::from_secs(5), async {
        let sock = Arc::new(
            UdpSocket::bind("0.0.0.0:0")
                .await
                .context("bind upstream udp socket")?,
        );
        sock.connect((target_host, target_port))
            .await
            .with_context(|| format!("connect upstream {target_host}:{target_port}"))?;
        // SSRF 二次防御:域名目标解析到内网地址则拒绝(堵 DNS rebinding / 内网域名)。
        if let Ok(peer) = sock.peer_addr() {
            crate::relay::guard_resolved_target(target_host, peer)?;
        }
        anyhow::Ok(sock)
    })
    .await
    {
        Ok(inner) => inner?,
        Err(_) => {
            counter.error_count.fetch_add(1, Ordering::Relaxed);
            warn!(rule_id, %client_addr, "udp upstream connect/DNS timed out; dropping new-session packet");
            return Ok(());
        }
    };
    counter.connection_count.fetch_add(1, Ordering::Relaxed);

    // 反向 task：从 upstream 收响应写回原 client_addr。
    let listener_clone = listener.clone();
    let upstream_clone = upstream.clone();
    let counter_clone = counter.clone();
    let bucket_back = bucket.clone();
    let upstream_task = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_PACKET];
        loop {
            match upstream_clone.recv(&mut buf).await {
                Ok(n) => {
                    // recv 即计 rx_bytes:被限速丢弃的响应包同样计入收到的流量。
                    counter_clone.rx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                    if let Some(b) = &bucket_back {
                        if !b.try_acquire(n) {
                            counter_clone.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    if let Err(e) = listener_clone.send_to(&buf[..n], client_addr).await {
                        warn!(rule_id, %client_addr, error = ?e, "udp send_to client error");
                        counter_clone.error_count.fetch_add(1, Ordering::Relaxed);
                        break;
                    }
                }
                Err(e) => {
                    warn!(rule_id, %client_addr, error = ?e, "udp upstream recv error");
                    counter_clone.error_count.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
        }
    });

    upstream
        .send(data)
        .await
        .context("upstream send (new session)")?;
    map.insert(
        client_addr,
        Session {
            upstream,
            last_seen: Instant::now(),
            upstream_task,
        },
    );
    Ok(())
}

async fn sweep_expired(
    rule_id: i64,
    sessions: &Arc<Mutex<HashMap<SocketAddr, Session>>>,
    session_timeout: Duration,
) {
    let mut map = sessions.lock().await;
    let now = Instant::now();
    let expired: Vec<SocketAddr> = map
        .iter()
        .filter(|(_, s)| now.duration_since(s.last_seen) > session_timeout)
        .map(|(a, _)| *a)
        .collect();
    for addr in &expired {
        if let Some(session) = map.remove(addr) {
            session.upstream_task.abort();
        }
    }
    if !expired.is_empty() {
        info!(
            rule_id,
            expired_count = expired.len(),
            "udp sessions swept"
        );
    }
}

#[cfg(test)]
async fn start_with(
    rule: Rule,
    stats: Arc<StatsCollector>,
    session_timeout: Duration,
    sweep_interval: Duration,
    max_sessions: usize,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<UdpRelayHandle> {
    start_inner(rule, stats, session_timeout, sweep_interval, max_sessions, bucket).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::StatsCollector;
    use emorelay_common::control::v1::Rule;
    use std::net::UdpSocket as StdUdpSocket;

    fn ephemeral_port() -> u16 {
        // UDP 抢端口同 TCP 套路:bind 0 → port → drop。
        StdUdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// spawn 一个 UDP echo server,返回端口。
    /// 收什么发回什么(send_to 原源地址)。
    async fn spawn_udp_echo_server() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    break;
                };
                let _ = socket.send_to(&buf[..n], peer).await;
            }
        });
        port
    }

    fn rule_for(listen_port: u16, target_port: u16) -> Rule {
        Rule {
            id: 7,
            protocol: "udp".into(),
            listen_ip: "127.0.0.1".into(),
            listen_port: listen_port as u32,
            target_host: "127.0.0.1".into(),
            target_port: target_port as u32,
            enabled: true,
            bandwidth_mbps: 0,
            max_connections: 0,
            blocked_protocols: 0,
            extra_targets: Vec::new(),
            lb_strategy: String::new(),
            tunnel: None,
        }
    }

    #[tokio::test]
    async fn udp_relay_round_trips_and_counts() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());

        let handle = start(rule_for(listen_port, echo_port), stats.clone(), None)
            .await
            .expect("relay start");
        tokio::time::sleep(Duration::from_millis(30)).await;

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client
            .send_to(b"ping", ("127.0.0.1", listen_port))
            .await
            .unwrap();

        let mut buf = [0u8; 64];
        let recv = tokio::time::timeout(Duration::from_millis(500), client.recv_from(&mut buf))
            .await
            .expect("recv timed out");
        let (n, _) = recv.unwrap();
        assert_eq!(&buf[..n], b"ping");

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await;

        let snap = stats.drain_snapshot();
        let s = snap
            .iter()
            .find(|s| s.rule_id == 7)
            .expect("expected stats for rule 7");
        assert_eq!(s.connection_count, 1, "expected 1 udp session");
        assert!(s.tx_bytes >= 4, "tx_bytes={}", s.tx_bytes);
        assert!(s.rx_bytes >= 4, "rx_bytes={}", s.rx_bytes);
        assert_eq!(s.error_count, 0);
    }

    /// 验证 session 闲置超过 session_timeout 后被 sweep:同 client port 第二次发包
    /// 触发新 session 建立, connection_count 应递增到 2。
    #[tokio::test]
    async fn udp_session_expires_after_timeout() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());

        // 用短超时,快速验证 sweep 路径。
        let handle = start_with(
            rule_for(listen_port, echo_port),
            stats.clone(),
            Duration::from_millis(200), // session_timeout
            Duration::from_millis(50),  // sweep_interval
            MAX_UDP_SESSIONS,
            None,
        )
        .await
        .expect("relay start");
        tokio::time::sleep(Duration::from_millis(30)).await;

        // 同一 client (固定 port) 发两轮包,中间隔过 session_timeout。
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut buf = [0u8; 64];

        client
            .send_to(b"first", ("127.0.0.1", listen_port))
            .await
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_millis(300), client.recv_from(&mut buf))
            .await
            .expect("first recv timed out")
            .unwrap();

        // 等过 session_timeout + 一个 sweep 周期 + 余量
        tokio::time::sleep(Duration::from_millis(400)).await;

        client
            .send_to(b"second", ("127.0.0.1", listen_port))
            .await
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_millis(300), client.recv_from(&mut buf))
            .await
            .expect("second recv timed out")
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop().await;

        let snap = stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 7).expect("stats");
        assert_eq!(
            s.connection_count, 2,
            "session 过期后第二次发包应建新 session, 计数 +1"
        );
    }

    /// I3:反向 task 已死的既有 session,下次 forward 应被移除并重建(恢复回程),
    /// 而非仅刷新 last_seen。手动构造一个 upstream_task 已 finished 的占位 session,
    /// 调一次 forward 后断言 session 被换成 task 仍存活的新实例,connection_count +1。
    #[tokio::test]
    async fn udp_dead_reverse_task_session_rebuilt() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let counter = stats.ensure(7);

        let listener = Arc::new(UdpSocket::bind(("127.0.0.1", listen_port)).await.unwrap());
        let sessions: Arc<Mutex<HashMap<SocketAddr, Session>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let client_addr: SocketAddr = "127.0.0.1:55555".parse().unwrap();

        // 构造一个反向 task 已死的占位 session:spawn 空 task 后自旋让出,
        // 直到 is_finished() 为 true(空 task 必然完成,自旋必然终止,确定性无 flaky)。
        // 不消费 handle,以便放进 Session。
        let dead_task = tokio::spawn(async {});
        while !dead_task.is_finished() {
            tokio::task::yield_now().await;
        }
        let stale_upstream = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let stale_local = stale_upstream.local_addr().unwrap();
        {
            let mut map = sessions.lock().await;
            map.insert(
                client_addr,
                Session {
                    upstream: stale_upstream,
                    // last_seen 设为很久以前,验证「重建」而非「靠 sweep」:即便没超时也应换掉。
                    last_seen: Instant::now(),
                    upstream_task: dead_task,
                },
            );
        }

        forward(
            7,
            &listener,
            client_addr,
            b"hello",
            &sessions,
            MAX_UDP_SESSIONS,
            "127.0.0.1",
            echo_port,
            &counter,
            &None,
        )
        .await
        .expect("forward");

        let map = sessions.lock().await;
        let s = map.get(&client_addr).expect("session 应仍存在(被重建)");
        assert!(
            !s.upstream_task.is_finished(),
            "重建后的反向 task 应存活"
        );
        assert_ne!(
            s.upstream.local_addr().unwrap(),
            stale_local,
            "upstream socket 应被换成新建的"
        );
        // 重建走新 session 分支,connection_count 从 0 递增到 1。
        assert_eq!(
            counter.connection_count.load(Ordering::Relaxed),
            1,
            "重建应计一次新连接"
        );

        // 收尾:abort 新 task 释放 socket。先释放 map 的不可变借用再操作。
        drop(map);
        let removed = sessions.lock().await.remove(&client_addr);
        if let Some(session) = removed {
            session.upstream_task.abort();
        }
    }

    /// NAT 表上限:max_sessions=2 时,第 3 个不同源的新 session 被拒(丢包计 error),
    /// connection_count 封顶在 2。既有 session 不受影响由其它测试覆盖。
    #[tokio::test]
    async fn udp_relay_caps_new_sessions_at_max() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        // 注入上限 2;长 timeout/interval 确保测试期间不过期/不 sweep。
        let handle = start_with(
            rule_for(listen_port, echo_port),
            stats.clone(),
            Duration::from_secs(60),
            Duration::from_secs(60),
            2,
            None,
        )
        .await
        .expect("relay start");
        tokio::time::sleep(Duration::from_millis(30)).await;

        // 3 个不同源(各自独立 client socket = 不同源端口)各发一包。
        let mut clients = Vec::new();
        for _ in 0..3 {
            let c = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            c.send_to(b"x", ("127.0.0.1", listen_port)).await.unwrap();
            clients.push(c);
        }
        // 给主循环时间逐个处理三包。
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.stop().await;

        let snap = stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 7).expect("stats");
        assert_eq!(s.connection_count, 2, "session 数应封顶在 max_sessions=2");
        assert!(s.error_count >= 1, "第 3 个新 session 被拒应计 error");
    }
}
