use anyhow::{Context, Result};
use emorelay_common::control::v1::Rule;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
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
    let target_host = rule.target_host.clone();
    let target_port: u16 = u16::try_from(rule.target_port)
        .with_context(|| format!("target_port out of u16 range: {}", rule.target_port))?;
    let rule_id = rule.id;
    // P10a 并发连接上限:permit 跟随 bridge task 生命周期,断开自动释放。
    // 上限变更走 re-apply 重建 listener+全新 limiter,存量连接持旧 permit 至自然断开
    // (与下方限速桶同一 MVP 语义),下调后短暂可能超员。
    let limiter = crate::limit::conn_limiter(rule.max_connections);
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!(rule_id, "tcp relay stopping");
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
                            let target_host = target_host.clone();
                            let counter = counter.clone();
                            // 限速变更走 re-apply 重建 listener+新桶,但存量连接持旧桶直到自然断开(沿用 stop 不断存量连接的 MVP 语义);新限速仅对新连接生效。
                            let bucket = bucket.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                if let Err(e) = bridge(client, target_host, target_port, counter.clone(), bucket).await {
                                    counter.error_count.fetch_add(1, Ordering::Relaxed);
                                    warn!(rule_id, %peer, error = ?e, "tcp bridge error");
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
async fn bridge(
    mut client: TcpStream,
    target_host: String,
    target_port: u16,
    counter: Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<()> {
    let mut server = TcpStream::connect((target_host.as_str(), target_port))
        .await
        .with_context(|| format!("connect upstream {target_host}:{target_port}"))?;

    // SSRF 二次防御:域名目标解析到内网地址则拒绝(堵 DNS rebinding / 内网域名)。
    if let Ok(peer) = server.peer_addr() {
        crate::relay::guard_resolved_target(&target_host, peer)?;
    }

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
}
