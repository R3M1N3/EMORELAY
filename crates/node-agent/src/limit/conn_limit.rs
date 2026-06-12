//! P10a 并发连接上限(仅 TCP)。Semaphore permit 跟随连接生命周期,
//! drop 自动释放;满了 try_acquire 失败 → 调用方直接断开新连接。
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// max_connections ≤ 0 = 不限(proto 用 0 表示 NULL)。
pub fn conn_limiter(max_connections: i64) -> Option<Arc<Semaphore>> {
    usize::try_from(max_connections)
        .ok()
        .filter(|n| *n > 0)
        .map(|n| Arc::new(Semaphore::new(n)))
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
