// crates/node-agent/src/limit/token_bucket.rs
//! per-rule token bucket(P2 限速)。rx+tx 共用一桶;tcp_udp 协议的 TCP/UDP 任务共享同一实例。
//! rate = bandwidth_mbps * 125_000 B/s;burst = max(rate/5, 65536)
//! (≈200ms 容量,下限 64KB 保证 UDP 最大单包可放行)。
//! 用 tokio::time::Instant,测试可用 start_paused 虚拟时钟。
//! 注意:acquire 不保证 FIFO 公平——并发等待者醒后重抢,可能交错分段。
//! tcp_udp 共桶时 TCP 的阻塞式 acquire 会持续消费回填 token,饱和时 UDP(try_acquire
//! 即查即丢)丢包率会显著高于均分直觉——这是 per-rule 总带宽语义的自然结果,非 bug。
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

pub struct TokenBucket {
    rate_bytes_per_sec: f64,
    burst_bytes: f64,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    /// mbps <= 0 → None(不限速)。
    pub fn from_mbps(mbps: i64) -> Option<Arc<Self>> {
        if mbps <= 0 {
            return None;
        }
        let rate = mbps as f64 * 125_000.0;
        let burst = (rate / 5.0).max(65536.0);
        Some(Arc::new(Self {
            rate_bytes_per_sec: rate,
            burst_bytes: burst,
            state: Mutex::new(BucketState {
                tokens: burst,
                last_refill: Instant::now(),
            }),
        }))
    }

    fn refill(&self, st: &mut BucketState) {
        let now = Instant::now();
        let dt = now.duration_since(st.last_refill).as_secs_f64();
        st.tokens = (st.tokens + dt * self.rate_bytes_per_sec).min(self.burst_bytes);
        st.last_refill = now;
    }

    /// TCP 路径:阻塞等待直到拿到 want 字节配额。
    /// want > burst 时分段按 burst 取,直到取满 want(防御死锁:桶容量上限是 burst,
    /// 单次申请不能超过它;TCP chunk 8KB 远小于 burst 下限 64KB,正常不走分段)。
    pub async fn acquire(&self, want: usize) {
        let mut remaining = want as f64;
        while remaining > 0.0 {
            let chunk = remaining.min(self.burst_bytes);
            loop {
                let wait = {
                    let mut st = self.state.lock().expect("token bucket poisoned");
                    self.refill(&mut st);
                    if st.tokens >= chunk {
                        st.tokens -= chunk;
                        break;
                    }
                    // 锁外 sleep,等缺口补齐
                    Duration::from_secs_f64((chunk - st.tokens) / self.rate_bytes_per_sec)
                };
                tokio::time::sleep(wait).await;
            }
            remaining -= chunk;
        }
    }

    /// UDP 路径:不阻塞;配额不足返回 false(调用方丢包计 error)。
    pub fn try_acquire(&self, want: usize) -> bool {
        let want = want as f64;
        let mut st = self.state.lock().expect("token bucket poisoned");
        self.refill(&mut st);
        if st.tokens >= want {
            st.tokens -= want;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn from_mbps_zero_means_unlimited() {
        assert!(TokenBucket::from_mbps(0).is_none());
        assert!(TokenBucket::from_mbps(-1).is_none());
        assert!(TokenBucket::from_mbps(10).is_some());
    }

    /// paused clock:8 Mbps = 1_000_000 B/s,burst = max(rate/5, 65536) = 200_000。
    /// 初始满桶,先吃掉 burst,再要 500_000 字节必须推进 ≈0.5s 虚拟时间。
    #[tokio::test(start_paused = true)]
    async fn acquire_waits_for_refill() {
        let b = TokenBucket::from_mbps(8).unwrap();
        b.acquire(200_000).await; // 清空 burst,不等待
        let start = tokio::time::Instant::now();
        b.acquire(500_000).await;
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(450) && waited <= Duration::from_millis(650),
            "expected ~0.5s virtual wait, got {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn try_acquire_fails_without_blocking_then_recovers() {
        let b = TokenBucket::from_mbps(8).unwrap();
        assert!(b.try_acquire(200_000), "满桶应放行");
        assert!(!b.try_acquire(100_000), "桶空应立即拒绝");
        tokio::time::advance(Duration::from_millis(200)).await; // 回填 200_000
        assert!(b.try_acquire(100_000));
    }

    /// 多任务共享同一桶:总放行速率受桶约束(虚拟时间)。
    #[tokio::test(start_paused = true)]
    async fn shared_bucket_serializes_concurrent_acquire() {
        let b: Arc<TokenBucket> = TokenBucket::from_mbps(8).unwrap();
        b.acquire(200_000).await; // 清空
        let start = tokio::time::Instant::now();
        let (b1, b2) = (b.clone(), b.clone());
        let t1 = tokio::spawn(async move { b1.acquire(250_000).await });
        let t2 = tokio::spawn(async move { b2.acquire(250_000).await });
        let _ = tokio::join!(t1, t2);
        let waited = start.elapsed();
        assert!(
            waited >= Duration::from_millis(400),
            "两个 250KB @1MB/s 应共等 ≥0.4s, got {waited:?}"
        );
    }
}
