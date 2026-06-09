// 限速/配额接口占位。plan §10 "带宽限制后续可用 token bucket 实现"的扩展点。
// 当前 Agent 侧不消费 Rule.bandwidth_mbps,本 trait 与 NullQuota 是接入位。
//
// 未来在 relay::tcp::bridge() 与 relay::udp::forward() 的 hot path 调用
//   guard.try_consume(n) 即可接入 token bucket;返回 None 时调用方决定
//   阻塞重试 / 丢弃 / 计入 error_count。

use std::sync::Arc;

pub trait QuotaGuard: Send + Sync {
    /// 申请 n 字节配额。返回 Some 允许; None 表示配额耗尽,调用方决策。
    /// MVP 实现 (NullQuota) 总是 Some。
    fn try_consume(&self, n: usize) -> Option<()>;
}

/// MVP 默认实现:不限速,所有 consume 直接 Some。
#[allow(dead_code)] // 占位:tcp::bridge() / udp::forward() 的 TODO(bandwidth) 注释里指向这里
pub struct NullQuota;

impl QuotaGuard for NullQuota {
    fn try_consume(&self, _n: usize) -> Option<()> {
        Some(())
    }
}

#[allow(dead_code)] // 同上,接入点见 relay/tcp.rs / relay/udp.rs 的 TODO(bandwidth)
pub fn null_quota() -> Arc<dyn QuotaGuard> {
    Arc::new(NullQuota)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_quota_always_allows() {
        let q = NullQuota;
        assert!(q.try_consume(0).is_some());
        assert!(q.try_consume(1024 * 1024).is_some());
    }
}
