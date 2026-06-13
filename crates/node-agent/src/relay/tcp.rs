use anyhow::{Context, Result};
use emorelay_common::control::v1::Rule;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::limit::TokenBucket;
use crate::stats::{RuleCounter, StatsCollector};

pub struct TcpRelayHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl TcpRelayHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        // 等待 listener task 自然终止；忽略 panic / cancel。
        let _ = self.join.await;
    }
}

pub async fn start(
    rule: Rule,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<TcpRelayHandle> {
    // 直接 (IpAddr, port) 构造 SocketAddr，避免 IPv6 字符串拼接歧义（"::1:8080" 不是合法 SocketAddr）。
    let listen_ip: IpAddr = rule
        .listen_ip
        .parse()
        .with_context(|| format!("invalid listen_ip: {}", rule.listen_ip))?;
    let listen_port: u16 = u16::try_from(rule.listen_port)
        .with_context(|| format!("listen_port out of u16 range: {}", rule.listen_port))?;
    let addr = SocketAddr::new(listen_ip, listen_port);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!(rule_id = rule.id, %addr, "tcp relay listening");

    let counter = stats.ensure(rule.id);
    // 多目标池:主目标(target_host:target_port)在前,extra_targets 追加在后。
    // 单目标(无 extra)→ 池长 1,选择恒返回主目标,行为与改造前一致。
    let mut pool: Vec<(String, u16)> = Vec::with_capacity(1 + rule.extra_targets.len());
    pool.push((
        rule.target_host.clone(),
        u16::try_from(rule.target_port)
            .with_context(|| format!("target_port out of u16 range: {}", rule.target_port))?,
    ));
    for t in &rule.extra_targets {
        match u16::try_from(t.port) {
            Ok(p) => pool.push((t.host.clone(), p)),
            Err(_) => warn!(rule_id = rule.id, port = t.port, "extra target port out of u16 range; skipped"),
        }
    }
    let pool = Arc::new(pool);
    let strategy = if rule.lb_strategy.is_empty() {
        "fifo".to_string()
    } else {
        rule.lb_strategy.clone()
    };
    // per-rule 轮询计数器(round/rand 用),所有连接共享。
    let rr = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rule_id = rule.id;
    // P10a 并发连接上限:permit 跟随 bridge task 生命周期,断开自动释放。
    // 上限变更走 re-apply 重建 listener+全新 limiter,存量连接持旧 permit 至自然断开
    // (与下方限速桶同一 MVP 语义),下调后短暂可能超员。
    let limiter = crate::limit::conn_limiter(rule.max_connections);
    // 节点级协议嗅探阻断掩码(0=不阻断);随 re-apply 重建 listener 生效。
    let blocked_protocols = rule.blocked_protocols;
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    // 取消闩:stop 时置 true,通知存量 bridge task 主动终止(断连=停止计费)。
    // 用 watch 而非 JoinSet/CancellationToken:仅需已启用的 tokio sync feature,零新依赖。
    let (cancel_tx, cancel_rx) = watch::channel(false);

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!(rule_id, "tcp relay stopping");
                    let _ = cancel_tx.send(true);
                    break;
                }
                res = listener.accept() => {
                    match res {
                        Ok((client, peer)) => {
                            let Ok(permit) = crate::limit::try_acquire(&limiter) else {
                                // 达上限:直接断开(drop socket = RST/FIN),计 error 供观测。
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                                warn!(rule_id, %peer, "connection rejected: max_connections reached");
                                continue;
                            };
                            counter.connection_count.fetch_add(1, Ordering::Relaxed);
                            let pool = pool.clone();
                            let strategy = strategy.clone();
                            let rr = rr.clone();
                            let counter = counter.clone();
                            // 限速变更走 re-apply 重建 listener+新桶,但存量连接持旧桶直到自然断开;新限速仅对新连接生效。
                            let bucket = bucket.clone();
                            let mut cancel_rx = cancel_rx.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                tokio::select! {
                                    // stop 触发:丢弃 bridge future,client/server socket 随之 drop 关闭。
                                    // wait_for 的 Err(sender 已 drop)亦视为取消,fail-safe 不会漏断。
                                    _ = async { let _ = cancel_rx.wait_for(|c| *c).await; } => {}
                                    r = bridge(client, &pool, &strategy, &rr, counter.clone(), bucket, blocked_protocols) => {
                                        if let Err(e) = r {
                                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                                            warn!(rule_id, %peer, error = ?e, "tcp bridge error");
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            error!(rule_id, error = ?e, "tcp accept error");
                        }
                    }
                }
            }
        }
    });

    Ok(TcpRelayHandle { stop_tx, join })
}

/// `(host: &str, port: u16)` 实现 `ToSocketAddrs`，自然处理 IPv4 / IPv6 / hostname；
/// 当 host 是裸 "::1" 时被识别为 IPv6 字面量，不走 DNS。
/// 多目标:按策略选起点,沿轮转顺序逐个 connect,首个成功即用(故障转移)。
async fn bridge(
    mut client: TcpStream,
    pool: &[(String, u16)],
    strategy: &str,
    rr: &std::sync::atomic::AtomicUsize,
    counter: Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
    blocked_protocols: u32,
) -> Result<()> {
    // 协议嗅探阻断:peek 首包(不消费,后续 splice/pump 仍能读到),命中被阻断协议则断连。
    // 客户端不先说话(2s 超时)则放行——无法指纹的流量不阻断(best-effort 防滥用)。
    // bail 走调用方的 error_count(被阻断连接计 error,可观测);此连接在 sniff 前已计
    // connection_count、已持 permit,最长占名额 2s(HTTP/TLS/SOCKS 首包即到,实际微秒级)。
    if blocked_protocols != 0 {
        let mut peekbuf = [0u8; 16];
        if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(2), client.peek(&mut peekbuf)).await {
            if let Some(proto) = crate::sniff::sniff_blocked(&peekbuf[..n], blocked_protocols) {
                anyhow::bail!("blocked protocol: {proto}");
            }
        }
    }

    // 按策略 + 客户端 IP 决定尝试顺序,逐个 connect 直到成功(其余作故障转移备选)。
    let client_ip = client
        .peer_addr()
        .map(|a| a.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let order = crate::select::target_order(pool.len(), strategy, client_ip, rr);
    let mut server: Option<TcpStream> = None;
    let mut last_err: Option<anyhow::Error> = None;
    // 多目标时给每次 connect 5s 上限,避免黑洞(DROP)的主目标拖到 OS TCP 超时才故障转移;
    // 单目标(池长 1)同样受益(挂死目标快速失败)。
    let connect_timeout = Duration::from_secs(5);
    for idx in order {
        let (host, port) = &pool[idx];
        let connected = match tokio::time::timeout(connect_timeout, TcpStream::connect((host.as_str(), *port))).await {
            Ok(r) => r,
            Err(_) => {
                last_err = Some(anyhow::anyhow!("connect upstream {host}:{port} timed out"));
                continue;
            }
        };
        match connected {
            Ok(s) => {
                // SSRF 二次防御:域名目标解析到内网地址则拒绝(堵 DNS rebinding / 内网域名)。
                if let Ok(peer) = s.peer_addr() {
                    if let Err(e) = crate::relay::guard_resolved_target(host, peer) {
                        last_err = Some(e);
                        continue;
                    }
                }
                server = Some(s);
                break;
            }
            Err(e) => {
                last_err = Some(anyhow::Error::new(e).context(format!("connect upstream {host}:{port}")));
            }
        }
    }
    let mut server = server
        .ok_or_else(|| last_err.unwrap_or_else(|| anyhow::anyhow!("no target reachable")))?;

    // Linux 不限速:走 splice 零拷贝,数据不过用户态(消除 pump 的两次 memcpy)。
    // 限速或非 Linux 回退下方 pump(用户态拷贝才能插入令牌桶计量)。
    // AGENT_RELAY_FORCE_PUMP=1 强制走 pump:仅用于 splice vs pump 的性能 A/B 对照。
    #[cfg(target_os = "linux")]
    if bucket.is_none() && !crate::relay::force_pump() {
        return crate::relay::splice::splice_bidi(client, server, counter).await;
    }

    let (mut c_r, mut c_w) = client.split();
    let (mut s_r, mut s_w) = server.split();

    // 字段命名约定：tx = client → target（发送出去），rx = target → client。
    // 不限速用 256KB 大缓冲,把高吞吐下的 read/write syscall 次数压到最低
    // (2Gbps 下 8KB 缓冲每秒数万次系统调用会烧满单核);限速路径用 64KB
    // (吞吐本就受令牌桶约束,过大缓冲无益)。
    let buf_size = if bucket.is_some() { 64 * 1024 } else { 256 * 1024 };
    let c2s = pump(&mut c_r, &mut s_w, &counter.tx_bytes, bucket.as_deref(), buf_size);
    let s2c = pump(&mut s_r, &mut c_w, &counter.rx_bytes, bucket.as_deref(), buf_size);
    tokio::try_join!(c2s, s2c)?;
    Ok(())
}

/// 单向拷贝:读 → (可选)向令牌桶取配额 → 写 → 计数。EOF 时半关写端。
/// buf_size 由调用方按是否限速选择,大缓冲是高吞吐下降低 CPU 的关键。
async fn pump<R, W>(
    r: &mut R,
    w: &mut W,
    counted: &std::sync::atomic::AtomicI64,
    bucket: Option<&TokenBucket>,
    buf_size: usize,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; buf_size];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(total);
        }
        if let Some(b) = bucket {
            b.acquire(n).await;
        }
        w.write_all(&buf[..n]).await?;
        counted.fetch_add(n as i64, Ordering::Relaxed);
        total += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::StatsCollector;
    use emorelay_common::control::v1::Rule;
    use std::net::TcpListener as StdTcpListener;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 借 std::net::TcpListener 拿一个 OS 分配的 ephemeral 端口然后 drop;
    /// 测试 race 概率小到可忽略(单机本进程内不会冲突)。
    fn ephemeral_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// spawn 一个 TCP echo server,返回端口。
    async fn spawn_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    fn rule_for(listen_port: u16, target_port: u16) -> Rule {
        Rule {
            id: 1,
            protocol: "tcp".into(),
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
    async fn tcp_relay_round_trips_and_counts_bytes() {
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());

        let handle = start(rule_for(listen_port, echo_port), stats.clone(), None)
            .await
            .expect("relay start");
        // 给 listener 几十毫秒就绪。
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        conn.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        // 半关写端触发 c2s 结束 → server 也 EOF → s2c 结束 → 双向 fetch_add。
        conn.shutdown().await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.stop().await;

        let snap = stats.drain_snapshot();
        let s = snap
            .iter()
            .find(|s| s.rule_id == 1)
            .expect("expected stats for rule 1");
        assert_eq!(s.connection_count, 1, "expected 1 connection");
        assert!(s.tx_bytes >= 5, "tx_bytes={}", s.tx_bytes);
        assert!(s.rx_bytes >= 5, "rx_bytes={}", s.rx_bytes);
        assert_eq!(s.error_count, 0, "should be no errors");
    }

    /// P10a 并发上限:max_connections=1 时第二个并发连接被立即断开,
    /// 第一个断开后名额释放、新连接恢复可用。
    #[tokio::test]
    async fn tcp_relay_enforces_max_connections() {
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let mut rule = rule_for(listen_port, echo_port);
        rule.max_connections = 1;

        let handle = start(rule, stats.clone(), None).await.expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 第一个连接占住名额(完成一次 echo 确认桥接已建立)。
        let mut c1 = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        c1.write_all(b"hold").await.unwrap();
        let mut buf = [0u8; 4];
        c1.read_exact(&mut buf).await.unwrap();

        // 第二个连接:accept 后立即被 drop,读端应见 EOF/RST 而非 echo。
        let mut c2 = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        let _ = c2.write_all(b"deny").await;
        let mut buf2 = [0u8; 4];
        let denied = match tokio::time::timeout(Duration::from_secs(2), c2.read(&mut buf2)).await {
            Ok(Ok(0)) => true,       // EOF
            Ok(Ok(_)) => false,      // 收到了 echo = 没被拒
            Ok(Err(_)) => true,      // RST
            Err(_) => false,         // 超时挂着 = 行为不对
        };
        assert!(denied, "second concurrent connection must be rejected");

        // 释放第一个连接 → 名额回收 → 新连接恢复。
        drop(c1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut c3 = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        c3.write_all(b"back").await.unwrap();
        let mut buf3 = [0u8; 4];
        c3.read_exact(&mut buf3).await.unwrap();
        assert_eq!(&buf3, b"back");

        handle.stop().await;
        let snap = stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 1).unwrap();
        assert_eq!(s.connection_count, 2, "rejected connection must not count");
        assert!(s.error_count >= 1, "rejection should be observable via error_count");
    }

    #[tokio::test]
    async fn tcp_relay_stop_is_idempotent_and_releases_port() {
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let handle = start(rule_for(listen_port, echo_port), stats.clone(), None)
            .await
            .expect("relay start");
        tokio::time::sleep(Duration::from_millis(30)).await;
        handle.stop().await;

        // stop 后再 bind 应该成功(端口已释放)。
        let _retake = TcpListener::bind(("127.0.0.1", listen_port))
            .await
            .expect("port should be released after stop");
    }

    /// 计费正确性:stop 必须主动断开存量连接,否则被禁用/删除的规则的长连接
    /// 会继续转发并继续计量。建一条活连接(echo 往返确认在桥),stop 后客户端
    /// 读应迅速返回 EOF/错误(连接被断),而非挂起到超时。
    #[tokio::test]
    async fn tcp_relay_stop_drops_inflight_connection() {
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let handle = start(rule_for(listen_port, echo_port), stats.clone(), None)
            .await
            .expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 建连并往返一次,确认 bridge 已活。
        let mut client = TcpStream::connect(("127.0.0.1", listen_port))
            .await
            .expect("connect");
        client.write_all(b"hi").await.expect("write");
        let mut echo = [0u8; 2];
        client.read_exact(&mut echo).await.expect("read echo");
        assert_eq!(&echo, b"hi");

        handle.stop().await;

        // stop 后存量连接必须被断:read 迅速以 EOF(0) 或错误返回。
        let mut buf = [0u8; 16];
        let r = tokio::time::timeout(Duration::from_secs(1), client.read(&mut buf))
            .await
            .expect("read should resolve quickly after stop, not hang");
        assert!(
            matches!(r, Ok(0)) || r.is_err(),
            "inflight connection must be closed after stop, got {r:?}"
        );
    }

    /// 限速生效:2 MB @ 40 Mbps(5 MB/s, burst 1 MB)理论 ≥(2MB-1MB)/5MB/s = 0.2s。
    /// 只断言下限(慢 CI 不误报);同时校验数据完整性。
    #[tokio::test]
    async fn tcp_relay_throttles_when_bucket_set() {
        use crate::limit::TokenBucket;
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let mut rule = rule_for(listen_port, echo_port);
        rule.bandwidth_mbps = 40;
        let bucket = TokenBucket::from_mbps(rule.bandwidth_mbps);
        let handle = start(rule, stats.clone(), bucket).await.expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let payload = vec![0xAB_u8; 2 * 1024 * 1024];
        let started = std::time::Instant::now();
        let mut conn = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        let writer = {
            let payload = payload.clone();
            async move {
                let (mut r, mut w) = conn.split();
                w.write_all(&payload).await.unwrap();
                w.shutdown().await.unwrap();
                let mut buf = Vec::with_capacity(payload.len());
                r.read_to_end(&mut buf).await.unwrap();
                buf
            }
        };
        let echoed = writer.await;
        let elapsed = started.elapsed();
        assert_eq!(echoed.len(), payload.len(), "数据必须完整");
        assert!(
            elapsed >= Duration::from_millis(180),
            "40Mbps 下 2MB 往返应明显被限速, got {elapsed:?}"
        );
        handle.stop().await;
    }

    /// 多目标故障转移:主目标不可达时,fifo 策略回落到备目标,连接仍成功往返。
    #[tokio::test]
    async fn tcp_relay_failover_to_extra_target() {
        use emorelay_common::control::v1::TargetEndpoint;
        let echo_port = spawn_echo_server().await; // 健康备目标
        // 主目标:绑后立即 drop 拿一个大概率拒绝连接的端口。
        let dead_port = {
            let l = StdTcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let mut rule = rule_for(listen_port, dead_port); // 主目标 = 死端口
        rule.lb_strategy = "fifo".into();
        rule.extra_targets = vec![TargetEndpoint {
            host: "127.0.0.1".into(),
            port: echo_port as u32,
        }];
        let handle = start(rule, stats.clone(), None).await.expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        conn.write_all(b"hi").await.unwrap();
        let mut buf = [0u8; 2];
        // 主目标 connect 失败 → 故障转移到备 echo,往返成功。
        // 超时给宽:某些平台 connect 到 refused 端口非即时。
        tokio::time::timeout(Duration::from_secs(8), conn.read_exact(&mut buf))
            .await
            .expect("failover 后应往返成功")
            .expect("read echo");
        assert_eq!(&buf, b"hi");
        handle.stop().await;
    }

    /// 协议嗅探:阻断 HTTP 时,发 HTTP 请求的连接被断(读到 EOF),普通流量放行。
    #[tokio::test]
    async fn tcp_relay_blocks_sniffed_protocol() {
        let echo_port = spawn_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());
        let mut rule = rule_for(listen_port, echo_port);
        rule.blocked_protocols = crate::sniff::BLOCK_HTTP;
        let handle = start(rule, stats.clone(), None).await.expect("relay start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // HTTP 请求 → 被嗅探阻断,连接断开,读不到回显。
        let mut http = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        http.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 8];
        let r = tokio::time::timeout(Duration::from_secs(2), http.read(&mut buf))
            .await
            .expect("read 应迅速返回(连接被断)");
        assert!(matches!(r, Ok(0)) || r.is_err(), "HTTP 连接应被断开, got {r:?}");

        // 普通(非 HTTP/TLS/SOCKS)流量正常转发。
        let mut ok = TcpStream::connect(("127.0.0.1", listen_port)).await.unwrap();
        ok.write_all(b"\x00\x01ping").await.unwrap();
        let mut echo = [0u8; 6];
        ok.read_exact(&mut echo).await.expect("普通流量应放行并回显");
        assert_eq!(&echo, b"\x00\x01ping");

        handle.stop().await;
    }
}
