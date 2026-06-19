//! 进程内 DNS 解析缓存(realm-parity 的轻量版,零新依赖)。
//!
//! 背景:此前每条新连接都走 `TcpStream::connect((host, port))` / `UdpSocket::connect((host,port))`,
//! 内部是阻塞 `getaddrinfo`、无缓存——域名目标高频建连时重复解析 + 占用 tokio 阻塞线程
//! (对标 realm 用 hickory 全局缓存解析器)。本模块包住 `getaddrinfo`(`spawn_blocking` 不堵
//! eventloop)并按固定 TTL 缓存结果,消除「每条连接重复解析」这一实际开销。
//!
//! 设计取舍(对个人/小规模转发务实):固定 TTL 窗口而非按 DNS 记录 TTL(目标 IP 极少变);
//! 仅正向缓存(不缓存失败);字面 IP 直通(不查 DNS、不进缓存);命中不阻塞,未命中/过期才解析。
//! 解析语义与改造前一致(同走 OS getaddrinfo,尊重 /etc/hosts、nsswitch),仅多一层缓存。

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 缓存有效期。目标 IP 极少变,60s 足够消除高频建连的重复解析,又不至于长期钉死过期记录。
const TTL: Duration = Duration::from_secs(60);
/// 缓存条目上限,防异常情况下(海量不同域名目标)无界增长。超限先清过期项,仍超则本次不缓存。
const MAX_ENTRIES: usize = 1024;

fn cache() -> &'static Mutex<HashMap<String, (Vec<IpAddr>, Instant)>> {
    static C: OnceLock<Mutex<HashMap<String, (Vec<IpAddr>, Instant)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(host: &str) -> Option<Vec<IpAddr>> {
    let map = cache().lock().ok()?;
    let (ips, at) = map.get(host)?;
    if at.elapsed() < TTL {
        Some(ips.clone())
    } else {
        None
    }
}

fn cache_put(host: &str, ips: &[IpAddr]) {
    let Ok(mut map) = cache().lock() else { return };
    if map.len() >= MAX_ENTRIES {
        map.retain(|_, (_, at)| at.elapsed() < TTL);
        if map.len() >= MAX_ENTRIES {
            return; // 仍超限:跳过缓存(不影响正确性,仅少一次命中)。
        }
    }
    map.insert(host.to_string(), (ips.to_vec(), Instant::now()));
}

/// 阻塞 getaddrinfo(端口任意,只取 IP)。在 spawn_blocking 内调用。
fn resolve_blocking(host: &str) -> io::Result<Vec<IpAddr>> {
    use std::net::ToSocketAddrs;
    Ok((host, 0u16).to_socket_addrs()?.map(|sa| sa.ip()).collect())
}

/// 解析目标 host 为 IP 列表(带 TTL 缓存)。字面 IP 直通;域名走缓存 / getaddrinfo。
/// 返回空列表表示解析成功但无地址(调用方按"无可达地址"处理)。
pub async fn resolve_target(host: &str) -> io::Result<Vec<IpAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]); // 字面 IP:不查 DNS、不进缓存。
    }
    if let Some(ips) = cache_get(host) {
        return Ok(ips);
    }
    let h = host.to_string();
    let ips = tokio::task::spawn_blocking(move || resolve_blocking(&h))
        .await
        .map_err(|e| io::Error::other(format!("resolve join error: {e}")))??;
    if !ips.is_empty() {
        cache_put(host, &ips);
    }
    Ok(ips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn literal_ip_passthrough_no_dns() {
        assert_eq!(
            resolve_target("127.0.0.1").await.unwrap(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            resolve_target("::1").await.unwrap(),
            vec!["::1".parse::<IpAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn localhost_resolves_to_loopback_and_caches() {
        // localhost 离线可解析(/etc/hosts);首次走 getaddrinfo,第二次命中缓存,结果一致且为回环。
        let first = resolve_target("localhost").await.unwrap();
        assert!(!first.is_empty(), "localhost 应解析出地址");
        assert!(first.iter().all(|ip| ip.is_loopback()), "localhost 应全为回环: {first:?}");
        let second = resolve_target("localhost").await.unwrap();
        assert_eq!(first, second, "第二次应命中缓存,结果一致");
    }
}
