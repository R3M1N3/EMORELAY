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

use crate::stats::{RuleCounter, StatsCollector};

/// 客户端空闲多久后清理 NAT session。
const SESSION_TIMEOUT: Duration = Duration::from_secs(120);
/// 多久跑一次 sweep。
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// 最大单包 UDP 字节（IPv4 64KB - header）。
const MAX_PACKET: usize = 65535;

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

pub async fn start(rule: Rule, stats: Arc<StatsCollector>) -> Result<UdpRelayHandle> {
    start_inner(rule, stats, SESSION_TIMEOUT, SWEEP_INTERVAL).await
}

async fn start_inner(
    rule: Rule,
    stats: Arc<StatsCollector>,
    session_timeout: Duration,
    sweep_interval: Duration,
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
                            counter.tx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                            if let Err(e) = forward(
                                rule_id,
                                &listener,
                                client_addr,
                                &buf[..n],
                                &sessions,
                                &target_host,
                                target_port,
                                &counter,
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
    target_host: &str,
    target_port: u16,
    counter: &Arc<RuleCounter>,
) -> Result<()> {
    // TODO(bandwidth): 在 send / fetch_add 之前接入 traits::QuotaGuard 做 token bucket
    //   限速 (plan §10);MVP 用 NullQuota 占位,不限速。
    let mut map = sessions.lock().await;
    if let Some(s) = map.get_mut(&client_addr) {
        s.last_seen = Instant::now();
        s.upstream
            .send(data)
            .await
            .context("upstream send (existing session)")?;
        return Ok(());
    }

    // 新 session：用临时端口的 upstream socket + connect 默认 peer。
    let upstream = Arc::new(
        UdpSocket::bind("0.0.0.0:0")
            .await
            .context("bind upstream udp socket")?,
    );
    upstream
        .connect((target_host, target_port))
        .await
        .with_context(|| format!("connect upstream {target_host}:{target_port}"))?;
    counter.connection_count.fetch_add(1, Ordering::Relaxed);

    // 反向 task：从 upstream 收响应写回原 client_addr。
    let listener_clone = listener.clone();
    let upstream_clone = upstream.clone();
    let counter_clone = counter.clone();
    let upstream_task = tokio::spawn(async move {
        let mut buf = vec![0u8; MAX_PACKET];
        loop {
            match upstream_clone.recv(&mut buf).await {
                Ok(n) => {
                    counter_clone.rx_bytes.fetch_add(n as i64, Ordering::Relaxed);
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
) -> Result<UdpRelayHandle> {
    start_inner(rule, stats, session_timeout, sweep_interval).await
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
        }
    }

    #[tokio::test]
    async fn udp_relay_round_trips_and_counts() {
        let echo_port = spawn_udp_echo_server().await;
        let listen_port = ephemeral_port();
        let stats = Arc::new(StatsCollector::new());

        let handle = start(rule_for(listen_port, echo_port), stats.clone())
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
}
