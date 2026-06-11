pub mod tcp;
pub mod udp;
// splice 零拷贝仅 Linux 可用,其它平台 tcp::bridge 回退 pump。
#[cfg(target_os = "linux")]
pub mod splice;
