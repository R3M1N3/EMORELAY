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
