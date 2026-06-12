pub mod node_offline;
pub mod stats_retention;
pub mod tunnel_creds;
pub mod user_quota;

/// 读取 sweeper 周期类 env(秒)。非法/缺失回落 default,并钳到 min 下限
/// (集成测试可调小,生产防误配成 0 打爆 CPU)。
pub(crate) fn env_secs(key: &str, default: u64, min: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
        .max(min)
}
