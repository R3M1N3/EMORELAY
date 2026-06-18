//! P10a 并发连接上限(仅 TCP)。Semaphore permit 跟随连接生命周期,
//! drop 自动释放;满了 try_acquire 失败 → 调用方直接断开新连接。
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// max_connections ≤ 0 = 不限(proto 用 0 表示 NULL)。
pub fn conn_limiter(max_connections: i64) -> Option<Arc<Semaphore>> {
    usize::try_from(max_connections)
        .ok()
        .filter(|n| *n > 0)
        // clamp 到 Semaphore::MAX_PERMITS:tokio 对超过该上界(usize::MAX>>3)的 permit 数直接 panic,
        // 而 max_connections 来自下发/导入/状态文件,畸形大值(手滑多打几位/坏数据)不能砖掉规则
        // apply——启动重放时这条 panic 会中断整个 agent 启动。clamp 后等效"实际无上限"(没有真实
        // 规则需要这么多并发连接),不改正常取值语义。
        .map(|n| Arc::new(Semaphore::new(n.min(Semaphore::MAX_PERMITS))))
}

/// 取一个连接名额。None limiter = 不限(恒成功);Some 且满 = Err。
pub fn try_acquire(limiter: &Option<Arc<Semaphore>>) -> Result<Option<OwnedSemaphorePermit>, ()> {
    match limiter {
        None => Ok(None),
        Some(s) => s.clone().try_acquire_owned().map(Some).map_err(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_or_negative_means_unlimited() {
        assert!(conn_limiter(0).is_none());
        assert!(conn_limiter(-1).is_none());
        assert!(try_acquire(&None).is_ok());
    }

    // i64::MAX 仅在 64 位上超过 Semaphore::MAX_PERMITS(本项目只发 x86_64);32 位上
    // usize::try_from 会先失败返回 None,断言不成立,故限定 64 位。
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn absurd_value_clamps_instead_of_panicking() {
        // i64::MAX 远超 tokio Semaphore::MAX_PERMITS;改前 Semaphore::new 会 panic 砖掉规则 apply。
        let l = conn_limiter(i64::MAX);
        assert!(l.is_some(), "畸形大值应被 clamp 成有效 limiter,而非 panic");
        assert!(try_acquire(&l).is_ok(), "clamp 后仍能正常取名额");
    }

    #[test]
    fn permits_enforce_cap_and_release_on_drop() {
        let l = conn_limiter(2);
        let p1 = try_acquire(&l).unwrap();
        let _p2 = try_acquire(&l).unwrap();
        assert!(try_acquire(&l).is_err(), "third concurrent connection must be rejected");
        drop(p1);
        assert!(try_acquire(&l).is_ok(), "slot must free up when a connection closes");
    }
}
