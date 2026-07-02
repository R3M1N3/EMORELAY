pub mod tcp;
pub mod udp;
// splice 零拷贝仅 Linux 可用,其它平台 tcp::bridge 回退 pump。
#[cfg(target_os = "linux")]
pub mod splice;

use std::net::IpAddr;

/// 性能 A/B 开关:`AGENT_RELAY_FORCE_PUMP=1` 时不限速 TCP 也走用户态 pump 而非 splice,
/// 用于真机对照 splice 与 pump 的吞吐。默认关(走 splice),不影响生产行为。
pub(crate) fn force_pump() -> bool {
    std::env::var("AGENT_RELAY_FORCE_PUMP")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 转发/隧道路径统一关闭 Nagle 算法。中转大量交互式小包(SSH/游戏/RPC)时 Nagle 的
/// "攒包"会叠加最高 ~40ms 延迟;转发器不应替业务做这个吞吐换延迟的权衡(realm 等默认关)。
/// 设置失败极罕见(通常 socket 已关),无害,忽略。
pub(crate) fn set_nodelay(stream: &tokio::net::TcpStream) {
    let _ = stream.set_nodelay(true);
}

/// 转发两端 TCP 连接都启用 keepalive。中转长空闲连接(SSH/数据库长连/WebSocket)经 NAT
/// 静默超时或对端崩溃时,无 keepalive 的半开连接既不转发也不释放、挂死占 fd
/// (realm 对入站 client + 出站 server 两侧都设)。time=空闲 30s 起首探,interval=每 10s 一次;
/// 探测次数用 OS 默认(跨平台 with_retries 不一致,省略)。设置失败极罕见(socket 已关),忽略。
pub(crate) fn set_keepalive(stream: &tokio::net::TcpStream) {
    let ka = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(30))
        .with_interval(std::time::Duration::from_secs(10));
    let _ = socket2::SockRef::from(stream).set_tcp_keepalive(&ka);
}

/// accept() 出错是否属"良性"。ECONNABORTED 表示对端在 accept 完成前就断开,是正常现象,
/// 不应计入 error_count 污染错误率(realm 对该错误亦 continue 不计错)。
pub(crate) fn accept_error_is_benign(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::ConnectionAborted
}

/// 对端地址是否为回环/内网/链路本地等不可对外路由的地址。
/// SSRF 二次防御用:panel 端只能校验字面 IP,域名解析发生在 Agent,需在此堵 DNS rebinding。
pub(crate) fn is_internal_addr(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local() // 169.254/16,含云元数据 169.254.169.254
                || v4.is_unspecified()
                || v4.is_broadcast()
                // 100.64.0.0/10 运营商级 NAT(CGNAT)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped(::ffff:127.0.0.1 等)按其 v4 判定,防映射地址绕过。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_addr(&IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 唯一本地地址
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 链路本地
        }
    }
}

/// 域名目标解析后的对端地址若落在内网则拒绝(字面 IP 目标由 panel 按角色校验,跳过)。
/// 返回 Err 表示应中断本次转发。
pub(crate) fn guard_resolved_target(target_host: &str, peer: std::net::SocketAddr) -> anyhow::Result<()> {
    // target_host 本身是字面 IP → panel 端已对字面内网按 owner 角色拦截,Agent 不重复判定
    // (admin 合法的 127.0.0.1 本机转发必须放行)。仅当目标是域名时校验解析结果。
    if target_host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if is_internal_addr(&peer.ip()) {
        anyhow::bail!(
            "目标域名 {target_host} 解析到内网地址 {},拒绝转发(SSRF 防御)",
            peer.ip()
        );
    }
    Ok(())
}

/// `accept()` 出错后的退避。fd 耗尽(EMFILE/ENFILE)或内核缓冲/内存不足
/// (ENOBUFS/ENOMEM)这类"资源暂时不足"错误若立即重试,会陷入 100% CPU 忙循环,
/// 且忙循环本身拖住系统、阻碍 fd 释放形成活锁——必须 sleep 一拍让系统喘息。
/// 其余错误(如 ECONNABORTED:对端在 accept 前已断)无需退避,立即接受下一个连接。
/// (参考 realm / 生产级 server 的 accept 错误处理思路,自研实现。)
pub(crate) async fn accept_backoff(err: &std::io::Error) {
    if is_resource_exhausted(err) {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[cfg(target_os = "linux")]
fn is_resource_exhausted(err: &std::io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
    )
}

#[cfg(not(target_os = "linux"))]
fn is_resource_exhausted(err: &std::io::Error) -> bool {
    // 非 linux(本地开发/测试;生产是 linux musl 静态二进制)无 libc errno 依赖。
    // EMFILE/ENFILE 在 std 暂无稳定 ErrorKind 映射,这里保守覆盖 OOM 即可。
    matches!(err.kind(), std::io::ErrorKind::OutOfMemory)
}

#[cfg(test)]
mod ssrf_tests {
    use super::*;
    use std::net::SocketAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn internal_v4_classified() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254", // 云元数据
            "0.0.0.0",
            "100.64.0.1", // CGNAT
        ] {
            assert!(is_internal_addr(&ip(s)), "{s} 应判内网");
        }
    }

    #[test]
    fn public_v4_not_internal() {
        for s in ["1.1.1.1", "8.8.8.8", "154.88.64.140", "99.1.2.3", "100.63.255.255"] {
            assert!(!is_internal_addr(&ip(s)), "{s} 不应判内网");
        }
    }

    #[test]
    fn internal_v6_and_mapped() {
        assert!(is_internal_addr(&ip("::1")));
        assert!(is_internal_addr(&ip("::")));
        assert!(is_internal_addr(&ip("fc00::1"))); // ULA
        assert!(is_internal_addr(&ip("fe80::1"))); // link-local
        assert!(is_internal_addr(&ip("::ffff:127.0.0.1"))); // IPv4-mapped 回环
        assert!(!is_internal_addr(&ip("2606:4700:4700::1111"))); // 公网 v6
    }

    #[test]
    fn literal_ip_target_skips_guard() {
        // 字面 IP 目标(含内网)由 panel 按角色校验,Agent 不二次拦截。
        let peer: SocketAddr = "127.0.0.1:5201".parse().unwrap();
        assert!(guard_resolved_target("127.0.0.1", peer).is_ok());
        assert!(guard_resolved_target("10.0.0.5", "10.0.0.5:80".parse().unwrap()).is_ok());
    }

    #[test]
    fn domain_resolving_internal_is_rejected() {
        // 域名解析到内网 → 拒绝(DNS rebinding / 内网域名)。
        let peer: SocketAddr = "127.0.0.1:5201".parse().unwrap();
        assert!(guard_resolved_target("evil.example.com", peer).is_err());
    }

    #[test]
    fn domain_resolving_public_is_allowed() {
        let peer: SocketAddr = "1.1.1.1:443".parse().unwrap();
        assert!(guard_resolved_target("cloudflare.com", peer).is_ok());
    }
}

/// 安全拼接 host:port 字符串。host 能 parse 为 IPv6 地址时格式化为 `[host]:port`，
/// 域名/IPv4 维持 `host:port`。消除 v6 字面量拼接歧义（`2001:db8::1:50100` 不是合法 SocketAddr）。
pub(crate) fn join_host_port(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// 按目标 IP 族选择 UDP 出站 bind 地址：v6 目标 bind `[::]:0`，v4 目标 bind `0.0.0.0:0`。
pub(crate) fn udp_bind_addr_for(ip: &IpAddr) -> &'static str {
    if ip.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    }
}

/// 按 remote_af 过滤解析到的 IP 列表。
/// "v4" → 只保留 IPv4；"v6" → 只保留 IPv6；"auto"/空串/其它 → 原样返回。
pub(crate) fn filter_af(ips: Vec<IpAddr>, remote_af: &str) -> Vec<IpAddr> {
    match remote_af {
        "v4" => ips.into_iter().filter(|ip| ip.is_ipv4()).collect(),
        "v6" => ips.into_iter().filter(|ip| ip.is_ipv6()).collect(),
        _ => ips,
    }
}

/// 双栈 TCP 监听：地址为 `0.0.0.0:{port}` 形态时，先尝试 `[::]:{port}` 并
/// 显式 `set_only_v6(false)` 使其同时接受 v4/v6；失败（内核禁用 v6 等）回退原地址。
/// 其余地址原样 bind。
pub(crate) async fn bind_tcp_dual(addr: &str) -> std::io::Result<tokio::net::TcpListener> {
    use std::net::SocketAddr;
    let sa: SocketAddr = addr.parse().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("parse bind addr {addr}: {e}"))
    })?;
    if sa.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED) {
        let port = sa.port();
        match try_bind_dual(port) {
            Ok(listener) => {
                tracing::info!(port, "tunnel hop bound [::] dual-stack (v4+v6)");
                return Ok(listener);
            }
            Err(e) => {
                tracing::warn!(port, error = %e, "dual-stack [::] bind failed, falling back to 0.0.0.0");
            }
        }
    }
    tokio::net::TcpListener::bind(addr).await
}

fn try_bind_dual(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_only_v6(false)?;
    socket.set_reuse_address(true)?;
    let bind_addr = std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, port, 0, 0);
    socket.bind(&SockAddr::from(bind_addr))?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    let std_listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(std_listener)
}

#[cfg(test)]
mod backoff_tests {
    use super::is_resource_exhausted;
    use std::io::{Error, ErrorKind};

    /// linux:仅 fd/缓冲/内存耗尽类 errno 触发退避;ECONNABORTED 等正常错误不退避。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_flags_only_resource_exhaustion() {
        assert!(is_resource_exhausted(&Error::from_raw_os_error(libc::EMFILE)));
        assert!(is_resource_exhausted(&Error::from_raw_os_error(libc::ENFILE)));
        assert!(is_resource_exhausted(&Error::from_raw_os_error(libc::ENOBUFS)));
        assert!(!is_resource_exhausted(&Error::from_raw_os_error(libc::ECONNABORTED)));
        assert!(!is_resource_exhausted(&Error::from(ErrorKind::Other)));
    }

    /// 非 linux:无 libc errno,仅 OOM 兜底触发,其它不退避。
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_oom_fallback_only() {
        assert!(is_resource_exhausted(&Error::from(ErrorKind::OutOfMemory)));
        assert!(!is_resource_exhausted(&Error::from(ErrorKind::ConnectionAborted)));
    }
}

#[cfg(test)]
mod keepalive_tests {
    use super::*;

    /// set_keepalive 后应真的启用 SO_KEEPALIVE(socket2 读回校验),且不 panic。
    #[tokio::test]
    async fn set_keepalive_enables_so_keepalive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        set_keepalive(&client);
        set_keepalive(&server);
        let sr = socket2::SockRef::from(&client);
        assert!(sr.keepalive().unwrap_or(false), "client keepalive 应启用");
    }

    /// 仅 ECONNABORTED 算良性,其它 accept 错误照常计入 error。
    #[test]
    fn econnaborted_is_benign_others_not() {
        use std::io::{Error, ErrorKind};
        assert!(accept_error_is_benign(&Error::from(ErrorKind::ConnectionAborted)));
        assert!(!accept_error_is_benign(&Error::from(ErrorKind::Other)));
        assert!(!accept_error_is_benign(&Error::from(ErrorKind::OutOfMemory)));
    }
}

#[cfg(test)]
mod dualstack_tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn join_host_port_v4() {
        assert_eq!(join_host_port("1.2.3.4", 80), "1.2.3.4:80");
    }

    #[test]
    fn join_host_port_v6() {
        assert_eq!(join_host_port("2001:db8::1", 443), "[2001:db8::1]:443");
    }

    #[test]
    fn join_host_port_domain() {
        assert_eq!(join_host_port("example.com", 8080), "example.com:8080");
    }

    #[test]
    fn join_host_port_v6_mapped() {
        assert_eq!(
            join_host_port("::ffff:127.0.0.1", 99),
            "[::ffff:127.0.0.1]:99"
        );
    }

    #[test]
    fn udp_bind_addr_for_v4() {
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert_eq!(udp_bind_addr_for(&ip), "0.0.0.0:0");
    }

    #[test]
    fn udp_bind_addr_for_v6() {
        let ip: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(udp_bind_addr_for(&ip), "[::]:0");
    }

    #[test]
    fn filter_af_v4_only() {
        let ips: Vec<IpAddr> = vec![
            "1.1.1.1".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
            "8.8.8.8".parse().unwrap(),
        ];
        let filtered = filter_af(ips, "v4");
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|ip| ip.is_ipv4()));
    }

    #[test]
    fn filter_af_v6_only() {
        let ips: Vec<IpAddr> = vec![
            "1.1.1.1".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
        ];
        let filtered = filter_af(ips, "v6");
        assert_eq!(filtered.len(), 1);
        assert!(filtered[0].is_ipv6());
    }

    #[test]
    fn filter_af_auto_keeps_all() {
        let ips: Vec<IpAddr> = vec![
            "1.1.1.1".parse().unwrap(),
            "2001:db8::1".parse().unwrap(),
        ];
        assert_eq!(filter_af(ips.clone(), "auto").len(), 2);
        assert_eq!(filter_af(ips, "").len(), 2);
    }

    #[test]
    fn filter_af_all_filtered_out() {
        let ips: Vec<IpAddr> = vec!["2001:db8::1".parse().unwrap()];
        assert!(filter_af(ips, "v4").is_empty());
    }

    #[tokio::test]
    async fn bind_tcp_dual_loopback() {
        let l = bind_tcp_dual("0.0.0.0:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        assert!(addr.port() > 0);
    }

    #[tokio::test]
    async fn bind_tcp_dual_specific_v4() {
        let l = bind_tcp_dual("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_loopback());
        assert!(addr.is_ipv4());
    }
}
