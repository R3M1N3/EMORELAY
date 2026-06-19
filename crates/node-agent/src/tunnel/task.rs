//! TunnelTask(P3b 数据面)。per rule 一个实例,按 TunnelContext.role 三形态:
//! - entry: 监听业务 listen_port(按 protocol 起 TCP/UDP),每个 TCP 连接/UDP session
//!   dial 下一跳,先写 1 字节 stream preamble 再桥接。限速与 rule_stats 只在 entry 计。
//! - mid:   transport.bind(self_inter_port) → accept → dial 下一跳 → 纯字节 bridge
//!   (preamble 随流原样经过,不拆)。
//! - exit:  transport.bind(self_inter_port) → accept → 读 preamble:
//!   TCP → TcpStream::connect 业务 target 直连 bridge;UDP → 拆帧 ↔ UDP socket。
//! stop 语义与 relay/tcp.rs 一致:停 listener,并通过取消闩(watch)主动断开存量
//! TCP 连接(断连=停止计费);UDP session 走 Drop-abort,随 loop future drop 清理。
use anyhow::{Context as _, Result};
use emorelay_common::control::v1::{Rule, TunnelContext, TunnelRole};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{interval, Instant, MissedTickBehavior};
use tracing::{info, warn};

use crate::limit::TokenBucket;
use crate::stats::{RuleCounter, StatsCollector};
use crate::tunnel::frame::{read_frame, write_frame, STREAM_TCP, STREAM_UDP};
use crate::tunnel::transport::{HANDSHAKE_TIMEOUT, TunnelConn, TunnelTransport};

/// UDP session 闲置回收阈值/扫描周期,与 relay/udp.rs 对齐。
const UDP_SESSION_TIMEOUT: Duration = Duration::from_secs(120);
const UDP_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const MAX_UDP_PACKET: usize = 65535;
/// 隧道 entry UDP NAT 表上限:每 session 占一条隧道连接(fd)+ writer/reader 两个 task。
/// 源地址可伪造,无上限会被海量伪造源耗尽 fd/内存;达上限丢新包(既有 session 不受影响)。
/// 4096 与直连 relay/udp.rs 对齐,为满载 fd/内存设一个明确上限(基线本无上限)。
const MAX_UDP_SESSIONS: usize = 4096;
/// mid/exit hop 同时在握手中的连接上限。握手已移出 accept loop(防队头阻塞),但需防
/// 半开连接洪泛无限 spawn 握手 task 耗 fd/task:超过即立即丢弃新连接。握手完成(成功/
/// 失败)即释放名额,已建立的转发连接不占用,故不限制正常并发转发吞吐。
const HOP_HANDSHAKE_CONCURRENCY: usize = 256;

pub struct TunnelTaskHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl TunnelTaskHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

pub async fn start(
    rule: Rule,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
    transport: Arc<dyn TunnelTransport>,
) -> Result<TunnelTaskHandle> {
    let ctx = rule.tunnel.clone().context("rule has no tunnel context")?;
    match TunnelRole::try_from(ctx.role) {
        Ok(TunnelRole::Entry) => start_entry(rule, ctx, stats, bucket, transport).await,
        Ok(TunnelRole::Mid) => start_relay_hop(rule.id, ctx, transport, HopMode::Mid).await,
        Ok(TunnelRole::Exit) => {
            let target_port = u16::try_from(rule.target_port)
                .with_context(|| format!("target_port out of u16 range: {}", rule.target_port))?;
            start_relay_hop(
                rule.id,
                ctx,
                transport,
                HopMode::Exit { target_host: rule.target_host.clone(), target_port },
            )
            .await
        }
        _ => anyhow::bail!("unspecified tunnel role for rule {}", rule.id),
    }
}

// ============= entry =============

async fn start_entry(
    rule: Rule,
    ctx: TunnelContext,
    stats: Arc<StatsCollector>,
    bucket: Option<Arc<TokenBucket>>,
    transport: Arc<dyn TunnelTransport>,
) -> Result<TunnelTaskHandle> {
    let listen_ip: IpAddr = rule
        .listen_ip
        .parse()
        .with_context(|| format!("invalid listen_ip: {}", rule.listen_ip))?;
    let listen_port = u16::try_from(rule.listen_port)
        .with_context(|| format!("listen_port out of u16 range: {}", rule.listen_port))?;
    let addr = SocketAddr::new(listen_ip, listen_port);
    let next_hop = format!("{}:{}", ctx.next_hop_addr, ctx.next_hop_inter_port);
    let counter = stats.ensure(rule.id);
    let rule_id = rule.id;

    let want_tcp = matches!(rule.protocol.as_str(), "tcp" | "tcp_udp");
    let want_udp = matches!(rule.protocol.as_str(), "udp" | "tcp_udp");

    let tcp_listener = if want_tcp {
        Some(TcpListener::bind(addr).await.with_context(|| format!("bind {addr}"))?)
    } else {
        None
    };
    let udp_socket = if want_udp {
        Some(Arc::new(
            UdpSocket::bind(addr).await.with_context(|| format!("udp bind {addr}"))?,
        ))
    } else {
        None
    };
    info!(rule_id, %addr, tunnel_id = ctx.tunnel_id, "tunnel entry listening");

    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    // 取消闩:stop 时主动断存量 TCP 连接,语义同 relay/tcp.rs。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let join = tokio::spawn(async move {
        // P10a 并发连接上限:split 已保证仅 entry 非 0(此处即 entry)。
        let limiter = crate::limit::conn_limiter(rule.max_connections);
        let tcp_loop = async {
            match tcp_listener {
                Some(l) => entry_tcp_loop(rule_id, l, &transport, &next_hop, &counter, &bucket, &limiter, &cancel_rx).await,
                None => std::future::pending().await,
            }
        };
        let udp_loop = async {
            match udp_socket {
                Some(s) => entry_udp_loop(rule_id, s, &transport, &next_hop, &counter, &bucket).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = &mut stop_rx => {
                info!(rule_id, "tunnel entry stopping");
                let _ = cancel_tx.send(true);
            }
            _ = tcp_loop => warn!(rule_id, "tunnel entry tcp loop ended unexpectedly"),
            _ = udp_loop => warn!(rule_id, "tunnel entry udp loop ended unexpectedly"),
        }
    });
    Ok(TunnelTaskHandle { stop_tx, join })
}

async fn entry_tcp_loop(
    rule_id: i64,
    listener: TcpListener,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: &Option<Arc<TokenBucket>>,
    limiter: &Option<Arc<tokio::sync::Semaphore>>,
    cancel_rx: &watch::Receiver<bool>,
) {
    loop {
        match listener.accept().await {
            Ok((client, peer)) => {
                let Ok(permit) = crate::limit::try_acquire(limiter) else {
                    counter.error_count.fetch_add(1, Ordering::Relaxed);
                    warn!(rule_id, %peer, "tunnel entry connection rejected: max_connections reached");
                    continue;
                };
                counter.connection_count.fetch_add(1, Ordering::Relaxed);
                let transport = transport.clone();
                let next_hop = next_hop.to_string();
                let counter = counter.clone();
                let bucket = bucket.clone();
                let mut cancel_rx = cancel_rx.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        // stop 触发:丢弃 bridge,client/隧道连接随之 drop 关闭。
                        _ = async { let _ = cancel_rx.wait_for(|c| *c).await; } => {}
                        r = entry_tcp_conn(client, transport, &next_hop, &counter, bucket) => {
                            if let Err(e) = r {
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                                warn!(rule_id, %peer, error = ?e, "tunnel entry tcp bridge error");
                            }
                        }
                    }
                });
            }
            Err(e) => {
                counter.error_count.fetch_add(1, Ordering::Relaxed);
                warn!(rule_id, error = ?e, "tunnel entry accept error");
                // fd/内存耗尽时退避,防 100% CPU 忙循环阻碍恢复。
                crate::relay::accept_backoff(&e).await;
            }
        }
    }
}

async fn entry_tcp_conn(
    mut client: TcpStream,
    transport: Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<()> {
    crate::relay::set_nodelay(&client);
    let mut tunnel = transport.dial(next_hop).await?;
    tunnel.write_all(&[STREAM_TCP]).await.context("write stream preamble")?;
    tunnel.flush().await.context("flush stream preamble")?;
    let (mut c_r, mut c_w) = client.split();
    let (mut t_r, mut t_w) = tokio::io::split(tunnel);
    // 命名对齐 relay/tcp.rs:tx = client → 隧道(发出),rx = 隧道 → client。
    let c2t = copy_counted(&mut c_r, &mut t_w, bucket.as_deref(), &counter.tx_bytes);
    let t2c = copy_counted(&mut t_r, &mut c_w, bucket.as_deref(), &counter.rx_bytes);
    tokio::try_join!(c2t, t2c)?;
    Ok(())
}

/// 自适应缓冲(不限速 256KB / 限速 64KB)复制 + 计数 + 可选限速。与 relay/tcp.rs::pump 同构,
/// 多了 bucket 可选分支;未合并以不动既有 relay hot path。EOF 时半关写端。
async fn copy_counted<R, W>(
    r: &mut R,
    w: &mut W,
    bucket: Option<&TokenBucket>,
    counted: &std::sync::atomic::AtomicI64,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // 与 relay/tcp.rs::pump 对齐:不限速用 256KB 大缓冲把高吞吐下的 read/write syscall 压到最低
    // (隧道不可能走 splice,copy_counted 是 entry TCP 唯一数据通道,缓冲大小直接决定 syscall 频率);
    // 限速路径用 64KB(吞吐本受令牌桶约束,过大无益)。
    let buf_size = if bucket.is_some() { 64 * 1024 } else { 256 * 1024 };
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
        // WSS 等消息缓冲 transport 需要逐块 flush 推出数据(tcp/tls 上是 no-op),否则小请求滞留缓冲死锁。
        w.flush().await?;
        counted.fetch_add(n as i64, Ordering::Relaxed);
        total += n as u64;
    }
}

struct UdpTunnelSession {
    /// 主 loop → writer task 的入站包通道;drop 即关闭隧道连接(写半 shutdown)。
    frame_tx: mpsc::Sender<Vec<u8>>,
    /// 回程 reader,持 listen socket 的 Arc;Drop 时 abort,见下。
    reader_task: JoinHandle<()>,
    last_seen: Instant,
}

/// reader 阻塞在 read_frame 且持有 listen socket 引用;若只靠对端关连接触发 EOF,
/// 对端迟迟不关(或正常 restart 的 FIN 竞态)会让 UDP listen 端口迟迟不释放,
/// 同端口 rebind 报 AddrInUse。Drop-abort 覆盖三条移除路径:sweep retain 淘汰、
/// Closed 移除、entry stop 时整个 sessions map 随 udp_loop drop。
/// writer 仍走 channel close 优雅退出(shutdown 写半通知 exit 端)。
impl Drop for UdpTunnelSession {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

/// per client_addr 一条隧道连接(NAT session 语义)。sessions 由本 loop 独占,无锁;
/// 过期 retain 丢弃 → frame_tx 关闭 → writer 退出 → 连接关 → reader EOF 退出,链式清理。
async fn entry_udp_loop(
    rule_id: i64,
    socket: Arc<UdpSocket>,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    counter: &Arc<RuleCounter>,
    bucket: &Option<Arc<TokenBucket>>,
) {
    let mut sessions: HashMap<SocketAddr, UdpTunnelSession> = HashMap::new();
    let mut buf = vec![0u8; MAX_UDP_PACKET];
    let mut sweep = interval(UDP_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
    sweep.tick().await;

    loop {
        tokio::select! {
            res = socket.recv_from(&mut buf) => match res {
                Ok((n, client_addr)) => {
                    // recv 即计 tx:被限速丢掉的包仍算"收到过"(与 relay/udp.rs 一致)。
                    counter.tx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                    if let Some(b) = bucket {
                        if !b.try_acquire(n) {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    if let Some(s) = sessions.get_mut(&client_addr) {
                        s.last_seen = Instant::now();
                        // 失败分两种语义:Full = writer 背压(连接仍活),丢这一包即可;
                        // Closed = writer 已退出(隧道连接死亡),必须移除 session——
                        // 否则持续来包会不断刷 last_seen,retain 永不淘汰,永久黑洞。
                        match s.frame_tx.try_send(buf[..n].to_vec()) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                                // 本包丢弃;下一包走新建 session 路径。
                                sessions.remove(&client_addr);
                            }
                        }
                        continue;
                    }
                    // NAT 表上限:达上限丢弃新源的包,防伪造源耗尽 fd/内存(既有 session 上面已 continue)。
                    if sessions.len() >= MAX_UDP_SESSIONS {
                        counter.error_count.fetch_add(1, Ordering::Relaxed);
                        warn!(rule_id, %client_addr, max = MAX_UDP_SESSIONS, "tunnel udp sessions at cap; dropping new-session packet");
                        continue;
                    }
                    // dial 限时:下一跳不可达时 TCP connect 可挂数十秒,而本 await 在主
                    // 事件循环内,会停摆该规则全部 UDP 流量与 sweep。彻底方案是 spawn
                    // 建联(但破坏 sessions 无锁设计),MVP 先用超时兜底。
                    match tokio::time::timeout(Duration::from_secs(5), open_udp_session(
                        rule_id, transport, next_hop, socket.clone(),
                        client_addr, counter.clone(), bucket.clone(),
                    )).await {
                        Ok(Ok((frame_tx, reader_task))) => {
                            counter.connection_count.fetch_add(1, Ordering::Relaxed);
                            if frame_tx.try_send(buf[..n].to_vec()).is_err() {
                                counter.error_count.fetch_add(1, Ordering::Relaxed);
                            }
                            sessions.insert(client_addr, UdpTunnelSession {
                                frame_tx,
                                reader_task,
                                last_seen: Instant::now(),
                            });
                        }
                        Ok(Err(e)) => {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            warn!(rule_id, %client_addr, error = ?e, "open udp tunnel session failed");
                        }
                        Err(_) => {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            warn!(rule_id, %client_addr, "open udp tunnel session timed out");
                        }
                    }
                }
                Err(e) => {
                    counter.error_count.fetch_add(1, Ordering::Relaxed);
                    warn!(rule_id, error = ?e, "tunnel entry udp recv error");
                    // 持续性 recv 错误(内核 ENOBUFS/ENOMEM)下立即重试会忙循环烧满单核;复用 accept
                    // 退避:仅资源耗尽类 errno sleep 一拍,瞬时错误不退避(与直连 relay/udp.rs 一致)。
                    crate::relay::accept_backoff(&e).await;
                }
            },
            _ = sweep.tick() => {
                let now = Instant::now();
                sessions.retain(|_, s| now.duration_since(s.last_seen) <= UDP_SESSION_TIMEOUT);
            }
        }
    }
}

/// 建 session:dial → preamble 0x02 → split。writer:mpsc → write_frame;
/// reader:read_frame → send_to(client) + rx 计数(回程同样过桶,不足丢弃)。
/// 返回 (frame_tx, reader JoinHandle):后者交给 UdpTunnelSession,Drop 时 abort。
async fn open_udp_session(
    rule_id: i64,
    transport: &Arc<dyn TunnelTransport>,
    next_hop: &str,
    listener: Arc<UdpSocket>,
    client_addr: SocketAddr,
    counter: Arc<RuleCounter>,
    bucket: Option<Arc<TokenBucket>>,
) -> Result<(mpsc::Sender<Vec<u8>>, JoinHandle<()>)> {
    let mut tunnel = transport.dial(next_hop).await?;
    tunnel.write_all(&[STREAM_UDP]).await.context("write stream preamble")?;
    tunnel.flush().await.context("flush stream preamble")?;
    let (mut t_r, mut t_w) = tokio::io::split(tunnel);
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(64);

    tokio::spawn(async move {
        while let Some(payload) = frame_rx.recv().await {
            if let Err(e) = write_frame(&mut t_w, &payload).await {
                warn!(rule_id, error = ?e, "udp tunnel write_frame error");
                break;
            }
        }
        let _ = t_w.shutdown().await;
    });

    let reader_task = tokio::spawn(async move {
        let mut fbuf = Vec::new();
        loop {
            match read_frame(&mut t_r, &mut fbuf).await {
                Ok(n) => {
                    counter.rx_bytes.fetch_add(n as i64, Ordering::Relaxed);
                    if let Some(b) = &bucket {
                        if !b.try_acquire(n) {
                            counter.error_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    if let Err(e) = listener.send_to(&fbuf[..n], client_addr).await {
                        counter.error_count.fetch_add(1, Ordering::Relaxed);
                        warn!(rule_id, %client_addr, error = ?e, "udp send_to client error");
                        break;
                    }
                }
                Err(_) => break, // EOF/对端关闭(含 session 过期链式清理)。
            }
        }
    });

    Ok((frame_tx, reader_task))
}

// ============= mid / exit =============

#[derive(Clone)]
enum HopMode {
    Mid,
    Exit { target_host: String, target_port: u16 },
}

async fn start_relay_hop(
    rule_id: i64,
    ctx: TunnelContext,
    transport: Arc<dyn TunnelTransport>,
    mode: HopMode,
) -> Result<TunnelTaskHandle> {
    let bind_addr = format!("0.0.0.0:{}", ctx.self_inter_port);
    let mut listener = transport.bind(&bind_addr).await?;
    let next_hop = format!("{}:{}", ctx.next_hop_addr, ctx.next_hop_inter_port);
    info!(rule_id, %bind_addr, tunnel_id = ctx.tunnel_id, role = ctx.role, "tunnel hop listening");

    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    // 取消闩:stop 时主动断存量 hop 连接(mid/exit 不计费,但悬挂连接会占端口/上游)。
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // 握手并发闸,见 HOP_HANDSHAKE_CONCURRENCY。
    let handshake_sem = Arc::new(tokio::sync::Semaphore::new(HOP_HANDSHAKE_CONCURRENCY));
    let join = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    info!(rule_id, "tunnel hop stopping");
                    let _ = cancel_tx.send(true);
                    break;
                }
                // accept_pending 只接 TCP、不握手:握手移到下方 per-conn task,避免慢/半开
                // 握手在 accept loop 内串行造成接入队头阻塞 DoS。
                res = listener.accept_pending() => match res {
                    Ok(pending) => {
                        // 握手名额:满则立即丢弃(pending 随 drop 关闭 TCP),防半开洪泛耗 fd/task。
                        let Ok(permit) = handshake_sem.clone().try_acquire_owned() else {
                            warn!(rule_id, "tunnel hop handshake slots full; dropping connection");
                            continue;
                        };
                        let transport = transport.clone();
                        let next_hop = next_hop.clone();
                        let mode = mode.clone();
                        let mut cancel_rx = cancel_rx.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = async { let _ = cancel_rx.wait_for(|c| *c).await; } => {}
                                r = async move {
                                    let conn = pending.handshake().await;
                                    // 握手结束(成功/失败)即释放名额,后续 bridge 不占握手并发。
                                    drop(permit);
                                    handle_hop_conn(conn?, transport, &next_hop, mode).await
                                } => {
                                    if let Err(e) = r {
                                        warn!(rule_id, error = ?e, "tunnel hop conn error");
                                    }
                                }
                            }
                        });
                    }
                    // accept_pending 现在只因底层 TCP accept 失败而出错(握手错误已移入 per-conn task)。
                    // 仅对 fd/内存耗尽类 io 错误退避,防 100% CPU 忙循环;其余立即继续。
                    Err(e) => {
                        warn!(rule_id, error = ?e, "tunnel hop accept error");
                        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                            crate::relay::accept_backoff(io_err).await;
                        }
                    }
                }
            }
        }
    });
    Ok(TunnelTaskHandle { stop_tx, join })
}

async fn handle_hop_conn(
    conn: TunnelConn,
    transport: Arc<dyn TunnelTransport>,
    next_hop: &str,
    mode: HopMode,
) -> Result<()> {
    match mode {
        HopMode::Mid => {
            // preamble 不拆,随字节流原样转发给下一跳。
            let upstream = transport.dial(next_hop).await?;
            bridge_raw(conn, upstream).await
        }
        HopMode::Exit { target_host, target_port } => {
            let mut conn = conn;
            let mut preamble = [0u8; 1];
            // 读 preamble 套超时:连入后(尤其裸 TCP transport 无握手认证)迟迟不发 preamble 字节的
            // 连接否则会永久挂在 read_exact 上,占住该 conn 及其 fd。
            tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.read_exact(&mut preamble))
                .await
                .context("read stream preamble timed out")?
                .context("read stream preamble")?;
            match preamble[0] {
                STREAM_TCP => {
                    // connect 套 5s 超时(与 relay/tcp.rs 一致):黑洞(DROP)目标否则会拖到 OS TCP
                    // 超时(数十秒~数分钟)才失败,期间占住入站隧道连接与 fd。
                    let upstream = tokio::time::timeout(
                        Duration::from_secs(5),
                        TcpStream::connect((target_host.as_str(), target_port)),
                    )
                    .await
                    .with_context(|| format!("connect target {target_host}:{target_port} timed out"))?
                    .with_context(|| format!("connect target {target_host}:{target_port}"))?;
                    // SSRF 二次防御:与 relay/tcp.rs 一致,校验解析结果非内网(堵域名 DNS rebinding)。
                    // panel 只能校验字面 IP,域名解析在出口节点本地发生;字面内网 IP 已被 panel 拦,
                    // 字面公网 IP 在此自动放行。隧道出口此前缺这道补偿控制(SSRF 可达出口内网/云元数据)。
                    if let Ok(peer) = upstream.peer_addr() {
                        crate::relay::guard_resolved_target(&target_host, peer)?;
                    }
                    crate::relay::set_nodelay(&upstream);
                    bridge_raw(conn, Box::new(upstream)).await
                }
                STREAM_UDP => exit_udp_conn(conn, &target_host, target_port).await,
                other => anyhow::bail!("unknown stream preamble: {other:#04x}"),
            }
        }
    }
}

/// 双向纯字节复制(不计数不限速——计量只在 entry)。EOF 时半关写端。
async fn bridge_raw(a: TunnelConn, b: TunnelConn) -> Result<()> {
    let (mut a_r, mut a_w) = tokio::io::split(a);
    let (mut b_r, mut b_w) = tokio::io::split(b);
    let a2b = async {
        let n = copy_raw(&mut a_r, &mut b_w).await;
        let _ = b_w.shutdown().await;
        n
    };
    let b2a = async {
        let n = copy_raw(&mut b_r, &mut a_w).await;
        let _ = a_w.shutdown().await;
        n
    };
    tokio::try_join!(a2b, b2a)?;
    Ok(())
}

/// mid/exit 中继的纯字节复制(不计数/不限速——计量只在 entry)。隧道不能走 splice,tokio::io::copy
/// 固定 ~8KB 在多 Gbps 下 syscall 成为瓶颈,会把 entry 的大缓冲优化抵消在 mid/exit。这里用 64KB:
/// 相比 8KB 已是 8× 的 syscall 削减;又不取 entry 的 256KB——mid/exit hop 连接无 max_connections 上限,
/// 256KB×双向(512KB/连接)会成无界内存放大器,64KB 折中。逐块 flush 兼容 WSS 消息缓冲语义。EOF 半关写端。
async fn copy_raw<R, W>(r: &mut R, w: &mut W) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(total);
        }
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
        total += n as u64;
    }
}

/// exit 端 UDP 帧流:拆帧 → UDP send;UDP recv → 打帧回写。
/// 任一方向断(隧道 EOF / udp 错误)即结束,UDP socket 随之释放。
async fn exit_udp_conn(conn: TunnelConn, target_host: &str, target_port: u16) -> Result<()> {
    let udp = UdpSocket::bind("0.0.0.0:0").await.context("bind exit udp socket")?;
    udp.connect((target_host, target_port))
        .await
        .with_context(|| format!("connect udp target {target_host}:{target_port}"))?;
    // SSRF 二次防御:与 relay/udp.rs 一致,校验解析结果非内网(堵域名 DNS rebinding)。
    if let Ok(peer) = udp.peer_addr() {
        crate::relay::guard_resolved_target(target_host, peer)?;
    }
    let (mut t_r, mut t_w) = tokio::io::split(conn);

    let inbound = async {
        let mut fbuf = Vec::new();
        loop {
            let n = match read_frame(&mut t_r, &mut fbuf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if udp.send(&fbuf[..n]).await.is_err() {
                return;
            }
        }
    };
    let outbound = async {
        let mut buf = vec![0u8; MAX_UDP_PACKET];
        loop {
            let n = match udp.recv(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if write_frame(&mut t_w, &buf[..n]).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        _ = inbound => {}
        _ = outbound => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::StatsCollector;
    use crate::tunnel::tcp_transport::TcpTransport;
    use emorelay_common::control::v1::{Rule, TunnelContext, TunnelRole};
    use std::net::TcpListener as StdTcpListener;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream, UdpSocket};

    fn ephemeral_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn spawn_echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        port
    }

    /// 构造带 tunnel 上下文的 Rule。entry 监听 listen_port;mid/exit 监听 self_inter;
    /// exit 的 target 是业务目标。无关字段给 0/空。
    pub(super) fn tunnel_rule(
        role: TunnelRole,
        ordinal: u32,
        protocol: &str,
        listen_port: u16,
        target_port: u16,
        self_inter: u16,
        next_inter: u16,
    ) -> Rule {
        Rule {
            id: 42,
            protocol: protocol.into(),
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
            tunnel: Some(TunnelContext {
                tunnel_id: 9,
                role: role as i32,
                next_hop_addr: "127.0.0.1".into(),
                next_hop_inter_port: next_inter as u32,
                self_inter_port: self_inter as u32,
                transport: "tcp".into(),
                self_ordinal: ordinal,
            }),
        }
    }

    #[tokio::test]
    async fn two_hop_tcp_roundtrip_counts_only_entry() {
        let echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let entry_stats = Arc::new(StatsCollector::new());
        let exit_stats = Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "tcp", 0, echo, exit_port, 0),
            exit_stats.clone(),
            None,
            t.clone(),
        )
        .await
        .expect("exit start");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, echo, 0, exit_port),
            entry_stats.clone(),
            None,
            t.clone(),
        )
        .await
        .expect("entry start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        conn.shutdown().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        entry.stop().await;
        exit.stop().await;

        let snap = entry_stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 42).expect("entry stats");
        assert_eq!(s.connection_count, 1);
        assert!(s.tx_bytes >= 5 && s.rx_bytes >= 5, "tx={} rx={}", s.tx_bytes, s.rx_bytes);
        // 计量只在 entry:exit 不得产生该 rule 的统计。
        assert!(
            exit_stats.drain_snapshot().is_empty(),
            "exit 不应计 rule stats(避免 server 端按 rule_id 重复累加)"
        );
    }

    /// 计费正确性(隧道侧):entry stop 必须主动断开存量隧道连接,否则被停用的
    /// 隧道规则的长连接继续转发并继续计量。建活连接(往返确认在桥)后 stop entry,
    /// 客户端读应迅速 EOF/错误返回,而非挂起到超时。
    #[tokio::test]
    async fn tunnel_entry_stop_drops_inflight_connection() {
        let echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "tcp", 0, echo, exit_port, 0),
            Arc::new(StatsCollector::new()),
            None,
            t.clone(),
        )
        .await
        .expect("exit start");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, echo, 0, exit_port),
            Arc::new(StatsCollector::new()),
            None,
            t.clone(),
        )
        .await
        .expect("entry start");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"hi").await.unwrap();
        let mut echo_buf = [0u8; 2];
        conn.read_exact(&mut echo_buf).await.unwrap();
        assert_eq!(&echo_buf, b"hi");

        entry.stop().await;
        exit.stop().await;

        let mut buf = [0u8; 16];
        let r = tokio::time::timeout(Duration::from_secs(1), conn.read(&mut buf))
            .await
            .expect("read should resolve quickly after entry stop, not hang");
        assert!(
            matches!(r, Ok(0)) || r.is_err(),
            "inflight tunnel connection must be closed after entry stop, got {r:?}"
        );
    }

    #[tokio::test]
    async fn three_hop_tcp_roundtrip_via_mid() {
        let echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let mid_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let stats = || Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 2, "tcp", 0, echo, exit_port, 0),
            stats(), None, t.clone(),
        ).await.expect("exit");
        let mid = start(
            tunnel_rule(TunnelRole::Mid, 1, "tcp", 0, echo, mid_port, exit_port),
            stats(), None, t.clone(),
        ).await.expect("mid");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, echo, 0, mid_port),
            stats(), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"three-hop").await.unwrap();
        let mut buf = [0u8; 9];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"three-hop");

        entry.stop().await;
        mid.stop().await;
        exit.stop().await;
    }

    #[tokio::test]
    async fn entry_stop_releases_listen_port() {
        let entry_port = ephemeral_port();
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp", entry_port, 1, 0, 1),
            Arc::new(StatsCollector::new()), None, t,
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(30)).await;
        entry.stop().await;
        TcpListener::bind(("127.0.0.1", entry_port))
            .await
            .expect("port should be released after stop");
    }

    async fn spawn_udp_echo_server() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else { break };
                let _ = socket.send_to(&buf[..n], peer).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn two_hop_udp_roundtrip_with_session_reuse() {
        let echo = spawn_udp_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let entry_stats = Arc::new(StatsCollector::new());
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "udp", 0, echo, exit_port, 0),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("exit");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "udp", entry_port, echo, 0, exit_port),
            entry_stats.clone(), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut buf = [0u8; 64];
        // 同一 client 发两包:第二包复用 session,connection_count 应保持 1。
        for payload in [b"ping-1" as &[u8], b"ping-2"] {
            client.send_to(payload, ("127.0.0.1", entry_port)).await.unwrap();
            let (n, _) = tokio::time::timeout(
                Duration::from_millis(800),
                client.recv_from(&mut buf),
            )
            .await
            .expect("udp recv timed out")
            .unwrap();
            assert_eq!(&buf[..n], payload);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.stop().await;
        exit.stop().await;

        let snap = entry_stats.drain_snapshot();
        let s = snap.iter().find(|s| s.rule_id == 42).expect("entry stats");
        assert_eq!(s.connection_count, 1, "同 client 两包应复用一条隧道 session");
        assert!(s.tx_bytes >= 12 && s.rx_bytes >= 12, "tx={} rx={}", s.tx_bytes, s.rx_bytes);
    }

    /// tcp_udp 协议:同一 entry 同时通 TCP 与 UDP(preamble 区分)。
    #[tokio::test]
    async fn tcp_udp_protocol_serves_both_over_tunnel() {
        let tcp_echo = spawn_echo_server().await;
        let exit_port = ephemeral_port();
        let entry_port = ephemeral_port();
        let t: Arc<dyn crate::tunnel::transport::TunnelTransport> = Arc::new(TcpTransport);

        // exit 的 udp 目标用同端口的 udp echo;tcp 目标用 tcp echo。
        // 简化:业务 target 都指向 tcp_echo 端口,UDP 单独再起 echo 并另建一对 task 验证
        // 会重复——这里只验证 TCP 流在 tcp_udp 协议下仍通,UDP 已由上个测试覆盖。
        let exit = start(
            tunnel_rule(TunnelRole::Exit, 1, "tcp_udp", 0, tcp_echo, exit_port, 0),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("exit");
        let entry = start(
            tunnel_rule(TunnelRole::Entry, 0, "tcp_udp", entry_port, tcp_echo, 0, exit_port),
            Arc::new(StatsCollector::new()), None, t.clone(),
        ).await.expect("entry");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut conn = TcpStream::connect(("127.0.0.1", entry_port)).await.unwrap();
        conn.write_all(b"dual").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"dual");

        entry.stop().await;
        exit.stop().await;
    }
}
