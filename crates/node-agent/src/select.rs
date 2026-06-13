//! 多目标负载均衡的目标选择(P2,对标 flux fifo/round/rand/hash)。
//! 给定目标池与策略,返回「尝试顺序」——第 1 个是策略选中的主目标,其余作故障转移备选。
//! 纯函数,便于测试;round-robin 计数器由调用方(per-rule)持有。
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 返回长度 n 的索引序列(0..n 的一个轮转),首项为策略选中的主目标。
/// n<=1 → [0..n)。策略:
/// - fifo(默认/未知):恒 [0,1,..](主目标优先,其余作备,主备故障转移)
/// - round:轮询起点 = 计数器自增 % n
/// - rand:起点混合计数器与客户端 IP(无外部 rand 依赖,负载分散即可)
/// - hash:起点 = 客户端 IP 哈希 % n(同源恒定同目标,会话粘性)
pub fn target_order(n: usize, strategy: &str, client_ip: IpAddr, rr: &AtomicUsize) -> Vec<usize> {
    if n <= 1 {
        return (0..n).collect();
    }
    let start = match strategy {
        "round" => rr.fetch_add(1, Ordering::Relaxed) % n,
        "rand" => {
            let c = rr.fetch_add(1, Ordering::Relaxed) as u64;
            (hash_ip(client_ip).wrapping_add(c) % n as u64) as usize
        }
        "hash" => (hash_ip(client_ip) % n as u64) as usize,
        _ => 0,
    };
    (0..n).map(|i| (start + i) % n).collect()
}

fn hash_ip(ip: IpAddr) -> u64 {
    let mut h = DefaultHasher::new();
    ip.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(a: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, a))
    }

    #[test]
    fn single_or_empty_pool() {
        let rr = AtomicUsize::new(0);
        assert_eq!(target_order(1, "round", ip(1), &rr), vec![0]);
        assert_eq!(target_order(0, "round", ip(1), &rr), Vec::<usize>::new());
    }

    #[test]
    fn fifo_always_primary_first() {
        let rr = AtomicUsize::new(0);
        // 多次调用恒 [0,1,2](主目标优先,其余作故障转移备选)。
        for _ in 0..5 {
            assert_eq!(target_order(3, "fifo", ip(7), &rr), vec![0, 1, 2]);
        }
    }

    #[test]
    fn round_robin_rotates_start() {
        let rr = AtomicUsize::new(0);
        assert_eq!(target_order(3, "round", ip(1), &rr), vec![0, 1, 2]);
        assert_eq!(target_order(3, "round", ip(1), &rr), vec![1, 2, 0]);
        assert_eq!(target_order(3, "round", ip(1), &rr), vec![2, 0, 1]);
        assert_eq!(target_order(3, "round", ip(1), &rr), vec![0, 1, 2]);
    }

    #[test]
    fn hash_is_stable_per_client() {
        let rr = AtomicUsize::new(0);
        let a = target_order(4, "hash", ip(42), &rr);
        let b = target_order(4, "hash", ip(42), &rr);
        assert_eq!(a, b, "同客户端 IP hash 结果应恒定(会话粘性)");
        // 是一个合法轮转:覆盖全部索引。
        let mut sorted = a.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }

    #[test]
    fn order_is_always_a_rotation_covering_all() {
        let rr = AtomicUsize::new(0);
        for strat in ["fifo", "round", "rand", "hash"] {
            let mut o = target_order(5, strat, ip(3), &rr);
            assert_eq!(o.len(), 5);
            o.sort_unstable();
            assert_eq!(o, vec![0, 1, 2, 3, 4], "{strat} 应覆盖全部目标");
        }
    }
}
