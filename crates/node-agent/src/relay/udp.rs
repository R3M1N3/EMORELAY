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
/// UDP NAT 表上限:每 session 占一个 upstream socket(fd + ephemeral 端口)与一个反向 task
/// (后者常驻一个 MAX_PACKET 接收缓冲)。源地址可伪造,无上限会被海量伪造源耗尽 fd/端口/内存;
/// 达上限丢新包(既有 session 不受影响)。4096 对个人/小规模转发足够,为满载内存/fd/端口设一个
/// 明确上限(基线本无上限);反向缓冲保持 MAX_PACKET 不截断大响应,故用限并发数而非缩缓冲来控内存。
const MAX_UDP_SESSIONS: usize = 4096;

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
                            // 持续性 recv 错误(如内核 ENOBUFS/ENOMEM)下立即重试会忙循环烧满单核;
                            // 复用 accept 退避:仅对资源耗尽类 errno sleep 一拍,瞬时错误(绝大多数)不退避。
                            crate::relay::accept_backoff(&e).await;
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
    forward_with_resolver(
        rule_id,
        listener,
        client_addr,
        data,
        sessions,
        max_sessions,
        target_host,
        target_port,
        counter,
        bucket,
        |host| async move { crate::dns::resolve_target(&host).await },
    )
    .await
}

/// `forward` 的可注入 resolver 版本:生产用 `crate::dns::resolve_target`,测试可注入
/// 慢/可控 resolver 以验证「建连期间不持 sessions 锁」与「并发首包去重」。
///
/// 锁策略(本次加固重点):DNS 解析 + connect(5s 超时)从前一版的「全程持 sessions 锁」
/// 移到**锁外**执行——慢/不可达 DNS 不再独占 sessions 锁(sweep / stop / 其它 client 的
/// 锁操作不被阻塞)。仅在两段**短临界区**内持锁:① 快路径(既有 session / 死 task 重建 / 上限判定),
/// ② 新建好的 session 双检插入。语义保留:SSRF guard、5s 超时、`max_sessions` 上限、多目标
/// 故障转移均不变,只是发生在锁外。
#[allow(clippy::too_many_arguments)]
async fn forward_with_resolver<F, Fut>(
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
    resolve: F,
) -> Result<()>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Vec<IpAddr>>>,
{
    // 限速:配额不足直接丢包(UDP 语义,不阻塞事件循环)。
    if let Some(b) = bucket {
        if !b.try_acquire(data.len()) {
            counter.error_count.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    // —— 临界区 ①(短):既有 session 快路径 / 死 task 重建 / 上限判定。建连前持锁,不跨 await DNS。
    {
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
    } // 锁在此释放;下面的 DNS+connect 全程不持锁。

    // —— 锁外建连:用临时端口的 upstream socket + connect 默认 peer。
    // 域名目标的 connect 走阻塞 getaddrinfo,慢/不可达 DNS 此前会独占 sessions 锁;现移到锁外。
    // 与隧道 UDP 路径(tunnel/task.rs)一致套 5s 超时;超时按错误丢包(UDP 语义,不阻塞循环)。
    let upstream = match tokio::time::timeout(Duration::from_secs(5), async {
        // DNS 缓存解析(字面 IP 直通);取第一个通过 SSRF 校验且 connect 成功的地址。
        let ips = resolve(target_host.to_string())
            .await
            .with_context(|| format!("resolve upstream {target_host}"))?;
        let mut last: Option<anyhow::Error> = None;
        for ip in ips {
            let sa = SocketAddr::new(ip, target_port);
            // SSRF 二次防御:域名解析到内网则拒(字面 IP 由 panel 按角色校验,guard 内部放行)。
            if let Err(e) = crate::relay::guard_resolved_target(target_host, sa) {
                last = Some(e);
                continue;
            }
            let sock = Arc::new(
                UdpSocket::bind("0.0.0.0:0")
                    .await
                    .context("bind upstream udp socket")?,
            );
            match sock.connect(sa).await {
                Ok(()) => return anyhow::Ok(sock),
                Err(e) => last = Some(anyhow::Error::new(e).context(format!("connect upstream {sa}"))),
            }
        }
        Err(last.unwrap_or_else(|| {
            anyhow::anyhow!("connect upstream {target_host}:{target_port}: 无可用地址")
        }))
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
    // 先发首包:connected UDP 的 send 在 connect 成功后极少失败,但若失败必须在 spawn 反向 task
    // 之前返回——否则反向 task(持 upstream 的 Arc)无人 abort,task+upstream fd 永久泄漏
    // (sweep/stop 只 abort map 内的 session,够不着尚未插入的它)。首包成功后再计 connection_count。
    upstream
        .send(data)
        .await
        .context("upstream send (new session)")?;

    // 反向 task：从 upstream 收响应写回原 client_addr。锁外先建好,稍后双检插入。
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

    // —— 临界区 ②(短):双检插入。建连期间若有并发首包已为同 client 建好 session,
    // 丢弃自己刚建好的(abort 反向 task,upstream Arc 随之 drop 释放 fd),复用既有 session 转发本包;
    // 若既有 session 的反向 task 已死,则换上自己新建的(恢复回程)。否则插入自己。
    // 注意:首包此前已发往上游成功,丢弃自己时该包不重发(UDP 语义可丢,且复用分支已成功投递过本包)。
    let mut map = sessions.lock().await;
    let existing_alive = map
        .get(&client_addr)
        .is_some_and(|s| !s.upstream_task.is_finished());
    if existing_alive {
        // 并发已建好且存活:丢弃自己,复用既有 session 转发本包。
        upstream_task.abort();
        if let Some(s) = map.get_mut(&client_addr) {
            s.last_seen = Instant::now();
            s.upstream
                .send(data)
                .await
                .context("upstream send (existing session, dedup)")?;
        }
        return Ok(());
    }
    // 上限二次判定:① 判定后锁已释放,并发建连期间 map 可能涨到上限(生产为单循环串行不会发生,
    // 但保持上限严格、不被「锁外建连」削弱)。已为死 session 占位的 key 是替换不增长,放行;
    // 否则达上限则丢弃自己刚建好的(abort 释放 fd/端口),计一次 error。
    let replacing_existing = map.contains_key(&client_addr);
    if !replacing_existing && map.len() >= max_sessions {
        upstream_task.abort();
        counter.error_count.fetch_add(1, Ordering::Relaxed);
        warn!(rule_id, %client_addr, max = max_sessions, "udp sessions at cap; dropping new-session packet");
        return Ok(());
    }
    // 无存活既有 session:换上自己。若存在死 session,其 task 已结束、其 upstream 随旧 Session
    // drop 释放,无需显式 abort。首包成功后才计一次新连接。
    counter.connection_count.fetch_add(1, Ordering::Relaxed);
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
            send_proxy_protocol: false,
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

    /// 建连期间(慢 DNS)不持 sessions 锁:client A 注入一个会阻塞 200ms 的 resolver 触发新建,
    /// 在它仍卡在 DNS 时(锁应已释放),主线程去抢同一把 sessions 锁——若锁被建连段持有则抢锁
    /// 会被拖到 A 完成(>=150ms);锁外建连下应几乎立即拿到锁(<100ms)。
    /// 旧版「全程持锁」会让本断言 FAIL。
    #[tokio::test]
    async fn udp_slow_dns_does_not_hold_session_lock() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let listener = Arc::new(UdpSocket::bind(("127.0.0.1", listen_port)).await.unwrap());
        let sessions: Arc<Mutex<HashMap<SocketAddr, Session>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stats = Arc::new(StatsCollector::new());
        let counter = stats.ensure(7);
        let client_a: SocketAddr = "127.0.0.1:40001".parse().unwrap();

        // 慢 resolver:解析前阻塞 200ms,再返回回环地址(connect 到本地 echo 必成功)。
        // target_host 用字面 IP 127.0.0.1:guard_resolved_target 对字面 IP 放行(由 panel 按角色校验),
        // 不触发「域名解析到内网」拦截;注入 resolver 仅用于控制 DNS 耗时,验证锁不被持有。
        let slow_resolve = |_host: String| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(vec!["127.0.0.1".parse::<IpAddr>().unwrap()])
        };

        let listener_a = listener.clone();
        let sessions_a = sessions.clone();
        let counter_a = counter.clone();
        let connecting = tokio::spawn(async move {
            forward_with_resolver(
                7,
                &listener_a,
                client_a,
                b"a",
                &sessions_a,
                MAX_UDP_SESSIONS,
                "127.0.0.1",
                echo_port,
                &counter_a,
                &None,
                slow_resolve,
            )
            .await
            .expect("forward A");
        });

        // 让 A 跑进锁外的慢 DNS。给足让 ① 临界区结束、进入 sleep。
        tokio::time::sleep(Duration::from_millis(40)).await;

        // 此刻 A 应正卡在锁外慢 DNS:抢同一把锁应几乎立即成功。
        let start = Instant::now();
        {
            let _guard = sessions.lock().await;
        }
        let waited = start.elapsed();
        assert!(
            waited < Duration::from_millis(100),
            "建连期间不应持 sessions 锁;抢锁耗时 {waited:?}(>=100ms 说明锁被建连段长期持有)"
        );
        assert!(
            !connecting.is_finished(),
            "A 应仍卡在慢 DNS(确认上面的快速抢锁发生在建连进行中,而非建连已完成之后)"
        );

        // 收尾:等 A 完成并清理它建好的 session(abort 反向 task 释放 fd)。
        connecting.await.unwrap();
        let removed = sessions.lock().await.remove(&client_a);
        if let Some(s) = removed {
            s.upstream_task.abort();
        }
    }

    /// 并发同 client 首包去重(双检生效):两个 forward 对同一 client_addr 并发建连,
    /// 最终 map 只保留一个 session,且只计一次 connection_count(另一个被锁内双检丢弃)。
    #[tokio::test]
    async fn udp_concurrent_first_packet_dedups() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let listener = Arc::new(UdpSocket::bind(("127.0.0.1", listen_port)).await.unwrap());
        let sessions: Arc<Mutex<HashMap<SocketAddr, Session>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stats = Arc::new(StatsCollector::new());
        let counter = stats.ensure(7);
        let client: SocketAddr = "127.0.0.1:40002".parse().unwrap();

        // 两路都注入相同的轻微延时 resolver,确保两者重叠在锁外建连、再竞争锁内双检。
        // target_host 用字面 IP(同上,放行 SSRF guard;resolver 仅控制并发建连重叠时序)。
        let mk_resolve = || {
            |_host: String| async move {
                tokio::time::sleep(Duration::from_millis(80)).await;
                Ok(vec!["127.0.0.1".parse::<IpAddr>().unwrap()])
            }
        };

        let mut handles = Vec::new();
        for _ in 0..2 {
            let listener_c = listener.clone();
            let sessions_c = sessions.clone();
            let counter_c = counter.clone();
            let resolve = mk_resolve();
            handles.push(tokio::spawn(async move {
                forward_with_resolver(
                    7,
                    &listener_c,
                    client,
                    b"dup",
                    &sessions_c,
                    MAX_UDP_SESSIONS,
                    "127.0.0.1",
                    echo_port,
                    &counter_c,
                    &None,
                    resolve,
                )
                .await
                .expect("forward dedup");
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let map = sessions.lock().await;
        assert_eq!(map.len(), 1, "并发同 client 首包应只保留一个 session");
        let s = map.get(&client).expect("唯一 session 应在");
        assert!(!s.upstream_task.is_finished(), "保留的 session 反向 task 应存活");
        assert_eq!(
            counter.connection_count.load(Ordering::Relaxed),
            1,
            "并发首包去重后只应计一次新连接"
        );
        assert_eq!(
            counter.error_count.load(Ordering::Relaxed),
            0,
            "去重不应计 error"
        );

        // 收尾:abort 反向 task 释放 fd。
        drop(map);
        let removed = sessions.lock().await.remove(&client);
        if let Some(s) = removed {
            s.upstream_task.abort();
        }
    }
}
