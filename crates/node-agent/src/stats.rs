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
}
