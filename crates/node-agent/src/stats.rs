use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy)]
pub struct CounterSnapshot {
    pub rule_id: i64,
    pub rx_bytes: i64,
    pub tx_bytes: i64,
    pub connection_count: i64,
    pub error_count: i64,
}

#[derive(Default)]
pub struct RuleCounter {
    pub rx_bytes: AtomicI64,
    pub tx_bytes: AtomicI64,
    pub connection_count: AtomicI64,
    pub error_count: AtomicI64,
}

/// 每个规则一个 RuleCounter，原子计数器；TCP 连接 hot path 直接在原子上操作，
/// 不再持锁。snapshot 用于周期上报（单元 L）。
#[derive(Default)]
pub struct StatsCollector {
    counters: RwLock<HashMap<i64, Arc<RuleCounter>>>,
}

impl StatsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取或建。注意先用读锁 fast-path，避免每次都拿写锁。
    pub fn ensure(&self, rule_id: i64) -> Arc<RuleCounter> {
        if let Some(c) = self.counters.read().unwrap().get(&rule_id).cloned() {
            return c;
        }
        let mut w = self.counters.write().unwrap();
        w.entry(rule_id)
            .or_insert_with(|| Arc::new(RuleCounter::default()))
            .clone()
    }

    /// 用 swap(0) 抽取当前窗口的累计计数，counter reset 为 0；下一个上报窗口从 0 起算。
    /// 单原子 swap 与 hot path 的 fetch_add 互不丢失数据：reset 瞬间未 commit 的 add
    /// 会留在新窗口的 0 上。
    pub fn drain_snapshot(&self) -> Vec<CounterSnapshot> {
        let map = self.counters.read().unwrap();
        let mut out = Vec::with_capacity(map.len());
        for (rule_id, counter) in map.iter() {
            let rx = counter.rx_bytes.swap(0, Ordering::Relaxed);
            let tx = counter.tx_bytes.swap(0, Ordering::Relaxed);
            let conn = counter.connection_count.swap(0, Ordering::Relaxed);
            let err = counter.error_count.swap(0, Ordering::Relaxed);
            if rx == 0 && tx == 0 && conn == 0 && err == 0 {
                continue;
            }
            out.push(CounterSnapshot {
                rule_id: *rule_id,
                rx_bytes: rx,
                tx_bytes: tx,
                connection_count: conn,
                error_count: err,
            });
        }
        out
    }

    /// 把一次 drain 出来的快照加回计数器(上报失败时调用),下个窗口补报,
    /// 消除「drain 清零→上报失败→数据丢失」的丢数窗口。逐条 ensure:drain 后规则
    /// 可能已被 remove,ensure 重建以免回填丢失;fetch_add 与 hot path 并发无损叠加。
    ///
    /// 权衡(均为「宁可偶发偏高也不丢数」,对计费面板更安全):
    /// - 回填的字节在下个窗口用新的 bucket_at_unix 上报,server 端配额累加是
    ///   `forward_rules.rx/tx` 总量自增(与 bucket 无关),总量守恒不丢不重;仅
    ///   rule_stats 明细序列的分钟归属有偏移(观感非计费)。
    /// - 「server 已 commit 但 ACK 丢失」时回填会让这批字节被再加一次(偏高)。
    ///   这是所有「按 RPC 结果决定是否清零」方案的固有局限,根治需幂等键(留待后续)。
    pub fn restore(&self, snapshot: &[CounterSnapshot]) {
        for s in snapshot {
            let c = self.ensure(s.rule_id);
            c.rx_bytes.fetch_add(s.rx_bytes, Ordering::Relaxed);
            c.tx_bytes.fetch_add(s.tx_bytes, Ordering::Relaxed);
            c.connection_count
                .fetch_add(s.connection_count, Ordering::Relaxed);
            c.error_count.fetch_add(s.error_count, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_adds_back_drained_counts() {
        let stats = StatsCollector::new();
        let c = stats.ensure(1);
        c.rx_bytes.fetch_add(100, Ordering::Relaxed);
        c.tx_bytes.fetch_add(200, Ordering::Relaxed);
        c.connection_count.fetch_add(3, Ordering::Relaxed);
        c.error_count.fetch_add(1, Ordering::Relaxed);

        // 模拟上报失败:drain 拿到快照后清零,再 restore 回填。
        let snap = stats.drain_snapshot();
        assert_eq!(snap.len(), 1);
        // drain 后计数器已清零。
        assert!(stats.drain_snapshot().is_empty(), "drain 应已清零");

        stats.restore(&snap);
        // 回填后下一次 drain 应拿回原值。
        let again = stats.drain_snapshot();
        let s = again.iter().find(|s| s.rule_id == 1).expect("rule 1");
        assert_eq!(s.rx_bytes, 100);
        assert_eq!(s.tx_bytes, 200);
        assert_eq!(s.connection_count, 3);
        assert_eq!(s.error_count, 1);
    }

    #[test]
    fn restore_rebuilds_removed_counter() {
        // drain 后规则被移除(remove 删 counter),restore 仍应重建并回填,不丢数。
        let stats = StatsCollector::new();
        let snap = vec![CounterSnapshot {
            rule_id: 42,
            rx_bytes: 7,
            tx_bytes: 8,
            connection_count: 1,
            error_count: 2,
        }];
        stats.restore(&snap);
        let again = stats.drain_snapshot();
        let s = again.iter().find(|s| s.rule_id == 42).expect("rule 42");
        assert_eq!(s.rx_bytes, 7);
        assert_eq!(s.tx_bytes, 8);
        assert_eq!(s.connection_count, 1);
        assert_eq!(s.error_count, 2);
    }

    #[test]
    fn restore_merges_with_new_window_increment() {
        // 核心语义:上报失败回填后,新窗口的 hot path 增量与回填值在同一计数器叠加,
        // 下个窗口 drain 一次性带走「旧+新」之和,不丢不漏。
        let stats = StatsCollector::new();
        let snap = vec![CounterSnapshot {
            rule_id: 5,
            rx_bytes: 100,
            tx_bytes: 200,
            connection_count: 1,
            error_count: 0,
        }];
        stats.restore(&snap);
        // 模拟新窗口 hot path 又来了流量。
        let c = stats.ensure(5);
        c.rx_bytes.fetch_add(10, Ordering::Relaxed);
        c.tx_bytes.fetch_add(20, Ordering::Relaxed);

        let again = stats.drain_snapshot();
        let s = again.iter().find(|s| s.rule_id == 5).expect("rule 5");
        assert_eq!(s.rx_bytes, 110, "回填 100 + 新增 10");
        assert_eq!(s.tx_bytes, 220, "回填 200 + 新增 20");
    }
}
