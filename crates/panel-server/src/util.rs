use std::net::IpAddr;
use std::str::FromStr;

/// 仅接受合法 IPv4 / IPv6 字符串作为 listen_ip。
pub fn is_valid_ip(s: &str) -> bool {
    IpAddr::from_str(s).is_ok()
}

/// target_host 允许 IPv4 / IPv6 / 形似主机名的字符串。
/// 主机名约束：长度 1-253，每段 1-63，仅 [A-Za-z0-9-]，段首尾不为 '-'。
pub fn is_valid_target_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if IpAddr::from_str(host).is_ok() {
        return true;
    }
    host.split('.').all(|seg| {
        !seg.is_empty()
            && seg.len() <= 63
            && !seg.starts_with('-')
            && !seg.ends_with('-')
            && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}
