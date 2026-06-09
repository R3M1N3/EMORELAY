use emorelay_common::control::v1::Command;
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::mpsc;

/// 每个在线 node 一个 channel。SubscribeCommands 建立时注册 sender，
/// 规则 CRUD 写 DB 后通过 dispatch 推命令。
/// 后注册的 sender 替换前者；旧 receiver 自然 drop 即 stream 终止。
#[derive(Default)]
pub struct CommandDispatcher {
    channels: RwLock<HashMap<i64, mpsc::UnboundedSender<Command>>>,
}

impl CommandDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, node_id: i64) -> mpsc::UnboundedReceiver<Command> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.channels.write().unwrap().insert(node_id, tx);
        rx
    }

    /// 推命令；node 不在线（无 channel）或 receiver 已关闭都返回 false。
    pub fn dispatch(&self, node_id: i64, cmd: Command) -> bool {
        let map = self.channels.read().unwrap();
        match map.get(&node_id) {
            Some(tx) => tx.send(cmd).is_ok(),
            None => false,
        }
    }

    pub fn unsubscribe(&self, node_id: i64) {
        self.channels.write().unwrap().remove(&node_id);
    }
}

// TODO(单元 L): SubscribeCommands stream 终止时（agent 重连 / server 推送 chan 关闭）
// 当前没有显式 unsubscribe，channels 表中残留指向 dropped receiver 的 sender。
// 下次 dispatch 时 send Err 才会暴露。需在 service.rs 用 Drop guard 或包装 Stream 显式清理。
