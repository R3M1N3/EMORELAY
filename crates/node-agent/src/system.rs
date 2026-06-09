//! 节点资源采样器。
//!
//! 封装 sysinfo 的可变状态,提供线程安全 sample 接口。
//! - `refresh_metrics` (heartbeat 10s tick):只刷 CPU/MEM/LOAD,不动 networks。
//! - `drain` (stats 60s tick):刷新全部并取走网络字节增量(自上次 drain 起)。
//!
//! 两者分离的目的:heartbeat 不能把 stats 窗口的 rx/tx 提前 drain 走,
//! 否则 node_stats 表的 bucket 会丢失 50/60 的流量。

use std::sync::Mutex;
use sysinfo::{Networks, System};

#[derive(Debug, Clone, Copy)]
pub struct SystemSnapshot {
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct SystemSample {
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub load_average: f64,
    /// 自上次 `drain` 调用累计的全网卡 rx 字节(排除回环口)。
    pub rx_bytes_delta: i64,
    pub tx_bytes_delta: i64,
}

pub struct SystemSampler {
    inner: Mutex<Inner>,
}

struct Inner {
    sys: System,
    networks: Networks,
    last_rx_total: u64,
    last_tx_total: u64,
    /// 首次 drain 是否已经吃掉初始 baseline。
    /// sysinfo 各平台对 `new_with_refreshed_list` 是否立即读取 OctetCount 没有统一保证,
    /// 不消化首次 baseline 的话,首个 bucket 的 rx/tx 会变成"开机以来累计字节",
    /// 直接污染 `node_stats` 与 `nodes.rx_bytes_total`(且后者无回滚通道)。
    primed: bool,
}

impl SystemSampler {
    pub fn new() -> Self {
        let mut sys = System::new();
        // sysinfo 文档明确首次 CPU 采样不准(需要至少 MINIMUM_CPU_UPDATE_INTERVAL ~200ms
        // 间隔才有意义)。10s heartbeat 间隔天然满足,首次 0 值在第二次 tick 即被覆盖。
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let mut networks = Networks::new_with_refreshed_list();
        // 显式 refresh 一次,尽量让 total_received/transmitted 在 new() 返回前就被填充。
        // 即便此处 total 仍是 0(平台差异),`primed=false` 也会让首次 drain 强制 delta=0。
        networks.refresh();
        let (rx, tx) = total_traffic(&networks);
        Self {
            inner: Mutex::new(Inner {
                sys,
                networks,
                last_rx_total: rx,
                last_tx_total: tx,
                primed: false,
            }),
        }
    }

    /// 只刷 CPU/MEM/LOAD,不消耗网络计数。
    /// 用于 heartbeat tick(高频低开销)。
    pub fn refresh_metrics(&self) -> SystemSnapshot {
        let mut g = self.inner.lock().expect("sysinfo sampler poisoned");
        g.sys.refresh_cpu_usage();
        g.sys.refresh_memory();
        snapshot_of(&g.sys)
    }

    /// 刷新全部,返回 CPU/MEM/LOAD 采样值 + 自上次 drain 起的 rx/tx 增量。
    /// 网络基线在调用结束时更新到当前 total,下次 drain 算的就是新增量。
    pub fn drain(&self) -> SystemSample {
        let mut g = self.inner.lock().expect("sysinfo sampler poisoned");
        g.sys.refresh_cpu_usage();
        g.sys.refresh_memory();
        // sysinfo 0.32 的 Networks::refresh 无参,只刷新已知接口的字节计数。
        // 新接口(如网卡热插拔)由 new_with_refreshed_list 时一次性发现,
        // MVP 期间节点的物理网卡列表是稳定的,不再调 refresh_list 重发现。
        g.networks.refresh();
        let snap = snapshot_of(&g.sys);
        let (rx_now, tx_now) = total_traffic(&g.networks);
        let (rx_delta, tx_delta) = if g.primed {
            // saturating_sub 防止网卡重启导致 total 倒退时 underflow。
            (
                rx_now.saturating_sub(g.last_rx_total) as i64,
                tx_now.saturating_sub(g.last_tx_total) as i64,
            )
        } else {
            // 首次 drain:把当前 total 作为真正的 baseline,本次 delta 强制 0,
            // 从第二次 drain 起才上报真实窗口增量。
            g.primed = true;
            (0, 0)
        };
        g.last_rx_total = rx_now;
        g.last_tx_total = tx_now;
        SystemSample {
            cpu_usage: snap.cpu_usage,
            memory_usage: snap.memory_usage,
            load_average: snap.load_average,
            rx_bytes_delta: rx_delta,
            tx_bytes_delta: tx_delta,
        }
    }
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

fn snapshot_of(sys: &System) -> SystemSnapshot {
    let cpu_usage = sys.global_cpu_usage() as f64;
    let memory_usage = if sys.total_memory() > 0 {
        (sys.used_memory() as f64 / sys.total_memory() as f64) * 100.0
    } else {
        0.0
    };
    // load_average 是静态接口;Windows 上返回全 0,sysinfo 文档已说明,接受。
    let load_average = System::load_average().one;
    SystemSnapshot {
        cpu_usage,
        memory_usage,
        load_average,
    }
}

fn total_traffic(networks: &Networks) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for (name, net) in networks {
        // 排除回环口:节点内部 relay loopback 不应被记为节点出网流量。
        if is_loopback(name) {
            continue;
        }
        rx += net.total_received();
        tx += net.total_transmitted();
    }
    (rx, tx)
}

fn is_loopback(name: &str) -> bool {
    // Linux: "lo"; macOS: "lo0"; Windows: "Loopback Pseudo-Interface 1" / "loopback*"。
    name == "lo"
        || name.starts_with("lo0")
        || name.eq_ignore_ascii_case("loopback")
        || name.starts_with("Loopback")
}
