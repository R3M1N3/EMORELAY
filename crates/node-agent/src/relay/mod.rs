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
