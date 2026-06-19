//! 协议嗅探阻断(P1,对标 flux 协议屏蔽)。被动匹配 TCP 连接首包指纹,命中被阻断
//! 的协议则断连——防普通端口转发被滥用为开放 HTTP/SOCKS 代理或套 CDN。
//! 节点级开关,默认关闭;仅普通 TCP relay 生效(隧道走自有 transport)。
//! 这是防滥用(被动指纹 + 断连),不主动扫描/攻击任何对象。

/// 阻断位掩码(与 server `nodes.block_protocols` / proto `Rule.blocked_protocols` 对齐)。
pub const BLOCK_HTTP: u32 = 1;
pub const BLOCK_TLS: u32 = 2;
pub const BLOCK_SOCKS: u32 = 4;

/// 已知 HTTP 方法(含尾空格)。is_http_request 用于首行命中,could_be_blocked 用于前缀可能性判断。
/// PRI = HTTP/2 明文(h2c)prior-knowledge 连接前导 "PRI * HTTP/2.0\r\n"(不补则 h2c 直连绕过阻断)。
const HTTP_METHODS: &[&[u8]] = &[
    b"GET ", b"POST ", b"PUT ", b"HEAD ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"TRACE ",
    b"CONNECT ", b"PRI ",
];

/// 对连接首包前若干字节做指纹匹配,返回命中的被阻断协议名(用于日志),否则 None。
/// `mask` 为 0 时恒 None(不阻断)。字节不足以判定时保守放行(best-effort 防滥用)。
pub fn sniff_blocked(first: &[u8], mask: u32) -> Option<&'static str> {
    if mask == 0 || first.is_empty() {
        return None;
    }
    if mask & BLOCK_TLS != 0 && is_tls_client_hello(first) {
        return Some("tls");
    }
    if mask & BLOCK_HTTP != 0 && is_http_request(first) {
        return Some("http");
    }
    if mask & BLOCK_SOCKS != 0 && is_socks(first) {
        return Some("socks");
    }
    None
}

/// TLS record: 首字节 0x16(handshake) + 0x03(主版本) + 次版本 0x00..=0x04。
fn is_tls_client_hello(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0x16 && b[1] == 0x03 && b[2] <= 0x04
}

/// HTTP 请求行:已知方法 + 空格起头。
fn is_http_request(b: &[u8]) -> bool {
    HTTP_METHODS.iter().any(|m| b.starts_with(m))
}

/// SOCKS:首字节 0x05(SOCKS5,后随 nmethods)或 0x04(SOCKS4,后随 CONNECT/BIND 命令 0x01/0x02)。
/// 用第二字节进一步约束以压低误报(任意二进制协议恰好 0x04/0x05 开头的概率)。
fn is_socks(b: &[u8]) -> bool {
    if b.len() < 2 {
        return false;
    }
    match b[0] {
        0x05 => b[1] >= 1, // SOCKS5 nmethods ≥1(u8 上界 255 恒成立)
        0x04 => b[1] == 0x01 || b[1] == 0x02, // SOCKS4 CONNECT/BIND
        _ => false,
    }
}

/// first 是否**可能**是某个启用的被阻断协议指纹的前缀。用于嗅探累积:首包分片下若当前字节
/// 已不可能凑成任何被阻断协议,即可立即放行,不必死等更多字节/截止——消除良性 client-speaks-first
/// 短首帧协议最长 2s 的延迟,同时保留对真实分片的累积识别。空 first(还没数据)视为"仍可能"。
pub fn could_be_blocked(first: &[u8], mask: u32) -> bool {
    if mask == 0 {
        return false;
    }
    if first.is_empty() {
        return true;
    }
    (mask & BLOCK_TLS != 0 && tls_prefix_possible(first))
        || (mask & BLOCK_HTTP != 0 && http_prefix_possible(first))
        || (mask & BLOCK_SOCKS != 0 && socks_prefix_possible(first))
}

/// b 是否可能是 TLS ClientHello 记录头(0x16,0x03,0x00..=0x04)的前缀(按已有字节逐位判)。
fn tls_prefix_possible(b: &[u8]) -> bool {
    (b.is_empty() || b[0] == 0x16) && (b.len() < 2 || b[1] == 0x03) && (b.len() < 3 || b[2] <= 0x04)
}

/// b 是否可能是某个 HTTP 方法行的前缀(b 是方法的前缀,或已含完整方法)。
fn http_prefix_possible(b: &[u8]) -> bool {
    HTTP_METHODS.iter().any(|m| m.starts_with(b) || b.starts_with(m))
}

/// b 是否可能是 SOCKS 握手前缀(首字节 0x04/0x05;不足 1 字节视为可能)。
/// 故意只看首字节(宽判,避免漏放分片 SOCKS):SOCKS 在 1 字节后即"定型",不像 TLS/HTTP 随字节收紧。
fn socks_prefix_possible(b: &[u8]) -> bool {
    b.is_empty() || b[0] == 0x05 || b[0] == 0x04
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_zero_never_blocks() {
        assert_eq!(sniff_blocked(b"GET / HTTP/1.1\r\n", 0), None);
    }

    #[test]
    fn detects_http_when_enabled() {
        assert_eq!(sniff_blocked(b"GET / HTTP/1.1\r\n", BLOCK_HTTP), Some("http"));
        assert_eq!(sniff_blocked(b"POST /x", BLOCK_HTTP), Some("http"));
        // 未开 http 位则放行。
        assert_eq!(sniff_blocked(b"GET / HTTP/1.1", BLOCK_TLS), None);
    }

    #[test]
    fn detects_tls_client_hello() {
        // 0x16 0x03 0x01 = TLS1.0 handshake record。
        assert_eq!(sniff_blocked(&[0x16, 0x03, 0x01, 0x00], BLOCK_TLS), Some("tls"));
        assert_eq!(sniff_blocked(&[0x16, 0x03, 0x03], BLOCK_TLS), Some("tls"));
        // 非 TLS 字节放行。
        assert_eq!(sniff_blocked(&[0x16, 0x09, 0x01], BLOCK_TLS), None);
    }

    #[test]
    fn detects_socks() {
        assert_eq!(sniff_blocked(&[0x05, 0x01, 0x00], BLOCK_SOCKS), Some("socks"));
        assert_eq!(sniff_blocked(&[0x04, 0x01, 0x00, 0x50], BLOCK_SOCKS), Some("socks"));
        // SOCKS4 第二字节非 CONNECT/BIND 放行。
        assert_eq!(sniff_blocked(&[0x04, 0x09], BLOCK_SOCKS), None);
    }

    #[test]
    fn detects_h2c_prior_knowledge_preface() {
        // h2c prior-knowledge 前导;不补 PRI 方法则 HTTP/2 明文直连绕过 HTTP 阻断。
        assert_eq!(sniff_blocked(b"PRI * HTTP/2.0\r\n", BLOCK_HTTP), Some("http"));
    }

    #[test]
    fn could_be_blocked_prefix_logic() {
        let all = BLOCK_HTTP | BLOCK_TLS | BLOCK_SOCKS;
        // 仍是可能前缀 → true(嗅探应继续累积):
        assert!(could_be_blocked(b"CONNE", BLOCK_HTTP)); // "CONNECT " 的前缀(m.starts_with(b) 分支)
        assert!(could_be_blocked(b"GET ", BLOCK_HTTP)); // 已含完整方法(b.starts_with(m) 分支)
        assert!(could_be_blocked(&[0x16, 0x03], BLOCK_TLS)); // TLS record 头前缀
        assert!(could_be_blocked(&[0x05], BLOCK_SOCKS)); // SOCKS5 首字节
        assert!(could_be_blocked(b"", all)); // 还没字节,任何都可能
        // 已不可能是任何被阻断协议前缀 → false(应立即放行,不死等):
        assert!(!could_be_blocked(b"XY", all)); // 非方法前缀/非 TLS/非 SOCKS 首字节
        assert!(!could_be_blocked(&[0x00, 0x01], all)); // 二进制良性流量
        assert!(!could_be_blocked(b"GEX", BLOCK_HTTP)); // "GE" 像前缀但 "GEX" 不是任何方法前缀
        // mask 语义:0 恒 false;只开 TLS 时 HTTP 前缀不算"可能"。
        assert!(!could_be_blocked(b"CONNE", 0));
        assert!(!could_be_blocked(b"CONNE", BLOCK_TLS));
    }

    #[test]
    fn benign_traffic_passes() {
        // 普通二进制/文本流量,所有位全开也不误杀。
        let all = BLOCK_HTTP | BLOCK_TLS | BLOCK_SOCKS;
        assert_eq!(sniff_blocked(b"hello world", all), None);
        assert_eq!(sniff_blocked(&[0x00, 0x01, 0x02], all), None);
    }

    #[test]
    fn multiple_bits_block_matching_one() {
        let all = BLOCK_HTTP | BLOCK_TLS | BLOCK_SOCKS;
        assert_eq!(sniff_blocked(b"GET /", all), Some("http"));
        assert_eq!(sniff_blocked(&[0x16, 0x03, 0x01], all), Some("tls"));
    }
}
