use std::net::IpAddr;
use std::str::FromStr;

/// 仅接受合法 IPv4 / IPv6 字符串作为 listen_ip。
pub fn is_valid_ip(s: &str) -> bool {
    IpAddr::from_str(s).is_ok()
}

/// target_host 允许 IPv4 / IPv6 / 形似主机名的字符串。
/// 主机名约束：长度 1-253，每段 1-63，仅 [A-Za-z0-9-]，段首尾不为 '-'，
/// 且顶级标签（最后一段）不得为纯数字——否则形似 IP 的无效输入（如 1.2.3 /
/// 12345 / 1.2.3.4.5）会被误判为合法主机名（合法 FQDN 的 TLD 必含非数字字符）。
pub fn is_valid_target_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if IpAddr::from_str(host).is_ok() {
        return true;
    }
    let mut last_seg = "";
    let seg_ok = host.split('.').all(|seg| {
        last_seg = seg;
        !seg.is_empty()
            && seg.len() <= 63
            && !seg.starts_with('-')
            && !seg.ends_with('-')
            && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    seg_ok && !last_seg.chars().all(|c| c.is_ascii_digit())
}

/// 目标是否指向回环 / 私有 / 链路本地 / 未指定地址。仅对字面 IP 生效——
/// 域名解析后指向内网的情况拦不住（解析发生在 agent 连接时），属已知局限。
/// 用于阻止非管理员用户借节点中转访问 Agent 机的回环/内网服务。
pub fn is_internal_target_ip(host: &str) -> bool {
    match IpAddr::from_str(host) {
        Ok(IpAddr::V4(v4)) => is_internal_v4(&v4),
        Ok(IpAddr::V6(v6)) => {
            // IPv4-mapped(::ffff:a.b.c.d)解包后按 IPv4 判,堵 ::ffff:10.0.0.1 这类绕过。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_v4(&v4);
            }
            // is_unique_local / is_unicast_link_local 在 rust 1.80 仍 unstable，手动按前缀判断。
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
        Err(_) => false,
    }
}

fn is_internal_v4(v4: &std::net::Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
}

/// LIKE 模式转义:把 \ % _ 转义为字面量,配合 `LIKE ? ESCAPE '\'` 使用,
/// 防用户输入通配符污染搜索语义。
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// 规范化用户输入的到期时间为 SQLite `datetime('now')` 同款格式（UTC 语义）。
/// 接受 "YYYY-MM-DDTHH:MM"(datetime-local) / "YYYY-MM-DDTHH:MM:SS" / "YYYY-MM-DD HH:MM:SS"。
/// 统一输出 "YYYY-MM-DD HH:MM:SS"，可与 datetime('now') 直接字符串比较。
pub fn normalize_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    const FORMATS: &[&str] = &["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"];
    for f in FORMATS {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(dt.format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_host_accepts_ip_and_real_hostnames() {
        for ok in ["1.2.3.4", "::1", "2001:4860:4860::8888", "example.com", "a.b.co",
                   "host-1.example.com", "sub.domain.example.org", "localhost"] {
            assert!(is_valid_target_host(ok), "应接受 {ok}");
        }
    }

    #[test]
    fn target_host_rejects_ip_shaped_garbage() {
        // 用户报的 bug:1.2.3 不是合法 IP,旧逻辑当主机名放行。TLD 纯数字一律拒绝。
        for bad in ["1.2.3", "12345", "1.2.3.4.5", "999.999.999.999", "", "-bad.com",
                    "bad-.com", "under_score.com", "a..b"] {
            assert!(!is_valid_target_host(bad), "应拒绝 {bad}");
        }
    }

    #[test]
    fn internal_target_ip_detects_private_and_loopback() {
        for internal in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1",
                         "169.254.1.1", "0.0.0.0", "::1", "::", "fc00::1", "fe80::1",
                         "::ffff:10.0.0.1", "::ffff:127.0.0.1"] {
            assert!(is_internal_target_ip(internal), "{internal} 应判为内网");
        }
    }

    #[test]
    fn internal_target_ip_passes_public_and_nonip() {
        // 公网 IP 与域名(非字面 IP)都不拦——域名指向内网是已知局限。
        for ok in ["1.2.3.4", "8.8.8.8", "2001:4860:4860::8888", "example.com", "1.2.3"] {
            assert!(!is_internal_target_ip(ok), "{ok} 不应判为内网");
        }
    }
}
