//! node-agent 库入口(P3c)。把会话循环暴露为可编程 API,
//! 供 panel-server e2e 测试 in-process 起真 agent;二进制 main 也走这里。
pub mod agent;
pub mod config;
pub mod control;
pub mod limit;
pub mod manager;
pub mod probe;
pub mod relay;
pub mod retry;
pub mod sniff;
pub mod stats;
pub mod store;
pub mod system;
pub mod tunnel;
pub mod upgrade;

pub use agent::run_agent;
