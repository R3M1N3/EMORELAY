use emorelay_common::control::v1::Command;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use tokio::sync::mpsc;

/// 每个在线 node 一个 channel。SubscribeCommands 建立时注册 sender;
/// 规则 CRUD 写 DB 后通过 dispatch 推命令。
///
/// 每次 subscribe 颁发一个递增 generation;Drop guard (见 service.rs::GuardedStream)
/// 在 stream 终止时调 unsubscribe_if(node_id, generation),只在 generation 仍匹配时移除,
/// 避免旧 stream 关闭误删新订阅。这是 plan §13 验收 #4 的稳态保障。
#[derive(Default)]
pub struct CommandDispatcher {
    channels: RwLock<HashMap<i64, ChannelEntry>>,
    next_gen: AtomicU64,
}

struct ChannelEntry {
    generation: u64,
    sender: mpsc::UnboundedSender<Command>,
}

impl CommandDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 node 的 receiver,同 node 已有 entry 会被替换 (旧 generation 失效)。
    /// 返回 (receiver, generation),调用方需把 generation 交给 Drop guard 用作 unsubscribe key。
    pub fn subscribe(&self, node_id: i64) -> (mpsc::UnboundedReceiver<Command>, u64) {
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed);
        self.channels
            .write()
            .unwrap()
            .insert(node_id, ChannelEntry { generation, sender: tx });
        (rx, generation)
    }

    /// 推命令;node 不在线 (无 channel) 或 receiver 已关闭都返回 false。
    pub fn dispatch(&self, node_id: i64, cmd: Command) -> bool {
        let map = self.channels.read().unwrap();
        match map.get(&node_id) {
            Some(entry) => entry.sender.send(cmd).is_ok(),
            None => false,
        }
    }

    /// 仅在 channels[node_id].generation == generation 时移除。
    /// 防止"旧 stream Drop 时新订阅已装上"造成的错杀。
    pub fn unsubscribe_if(&self, node_id: i64, generation: u64) {
        let mut map = self.channels.write().unwrap();
        if let Some(entry) = map.get(&node_id) {
            if entry.generation == generation {
                map.remove(&node_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{command::Body, ApplyRule, Command};

    fn dummy_cmd() -> Command {
        Command {
            body: Some(Body::ApplyRule(ApplyRule { rule: None })),
        }
    }

    #[tokio::test]
    async fn subscribe_dispatch_unsubscribe_basic() {
        let d = CommandDispatcher::new();
        let (mut rx, gen) = d.subscribe(1);
        assert!(d.dispatch(1, dummy_cmd()), "dispatch 应送到");
        assert!(rx.recv().await.is_some());
        d.unsubscribe_if(1, gen);
        assert!(!d.dispatch(1, dummy_cmd()), "unsubscribe 后不应再送到");
    }

    #[tokio::test]
    async fn unsubscribe_if_does_not_kill_newer_generation() {
        let d = CommandDispatcher::new();
        let (_rx_old, gen_old) = d.subscribe(7);
        let (mut rx_new, _gen_new) = d.subscribe(7); // 替换旧 entry
        // 旧 stream 之后 Drop,但 generation 已不匹配,不应误删新订阅
        d.unsubscribe_if(7, gen_old);
        assert!(d.dispatch(7, dummy_cmd()), "新订阅必须仍然可达");
        assert!(rx_new.recv().await.is_some());
    }

    #[tokio::test]
    async fn dispatch_offline_node_returns_false() {
        let d = CommandDispatcher::new();
        assert!(!d.dispatch(42, dummy_cmd()));
    }
}
