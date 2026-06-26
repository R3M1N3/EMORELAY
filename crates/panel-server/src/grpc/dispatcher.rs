use emorelay_common::control::v1::Command;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// 每个在线 node channel 的容量。慢/假死 agent 时,待下发命令最多在 panel 内存
/// 堆积 CHANNEL_CAPACITY 条;超过即由 try_send 背压拒绝(dispatch 返回 false),
/// 调用方按「未送达」处理(warn + 重连 reconcile 自愈),不阻塞调用线程、不无限增长。
const CHANNEL_CAPACITY: usize = 1024;

/// 每个在线 node 一个有界 channel。SubscribeCommands 建立时注册 sender;
/// 规则 CRUD 写 DB 后通过 dispatch 推命令。
///
/// 每次 subscribe 颁发一个递增 generation;Drop guard (见 service.rs::GuardedStream)
/// 在 stream 终止时调 unsubscribe_if(node_id, generation),只在 generation 仍匹配时移除,
/// 避免旧 stream 关闭误删新订阅。这是 plan §13 验收 #4 的稳态保障。
#[derive(Default)]
pub struct CommandDispatcher {
    channels: RwLock<HashMap<i64, ChannelEntry>>,
    next_gen: AtomicU64,
    /// per-node 串行锁注册表(Gap #2)。reconcile 的「快照读 + 重放下发」与 delete 的
    /// 「软删 + RemoveRule 下发」对同一 node 经 [`lock_nodes`] 持同一把异步互斥,二者串行,
    /// 消除「delete 后被 reconcile 按旧快照(含已删 rule 的 keep_ids)复活」的窗口。
    /// 粒度 per-node:不同 node 互不阻塞,纯控制面,不影响转发热路径。
    /// 外层 std `Mutex` 仅护注册表查改(无 await,瞬时);每个 value 是 per-node 的
    /// 异步 `Mutex`,可跨 await 持有。entry 只增不删(node 数有界,内存可忽略)。
    node_locks: Mutex<HashMap<i64, Arc<AsyncMutex<()>>>>,
}

struct ChannelEntry {
    generation: u64,
    sender: mpsc::Sender<Command>,
}

impl CommandDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 node 的 receiver,同 node 已有 entry 会被替换 (旧 generation 失效)。
    /// 返回 (receiver, sender, generation):
    /// - receiver 交给 gRPC stream;
    /// - sender 是同一 channel 的发送端克隆,供 reconcile 重放用 `send().await` 背压式
    ///   下发(有界但**不丢**,因为 stream 已开始消费),避免在尚无消费者时同步顶满有界
    ///   channel 丢弃尾部命令(含权威 ReconcileRules);
    /// - generation 交给 Drop guard 用作 unsubscribe key。
    pub fn subscribe(&self, node_id: i64) -> (mpsc::Receiver<Command>, mpsc::Sender<Command>, u64) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed);
        self.channels
            .write()
            .unwrap()
            .insert(node_id, ChannelEntry { generation, sender: tx.clone() });
        (rx, tx, generation)
    }

    /// 推命令;返回 false 表示未送达:node 不在线 (无 channel)、receiver 已关闭,
    /// 或 channel 已满(慢/假死 agent 背压)。用 try_send 非阻塞——满即拒绝,
    /// 绝不阻塞调用线程(sync 上下文)、也不在 panel 内存无限堆积命令。
    pub fn dispatch(&self, node_id: i64, cmd: Command) -> bool {
        let map = self.channels.read().unwrap();
        match map.get(&node_id) {
            Some(entry) => entry.sender.try_send(cmd).is_ok(),
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

    /// 取得给定节点集合的 per-node 串行锁,返回持有的 owned guards(随返回值 drop 释放)。
    /// 调用方在锁的生命周期内完成「reconcile 快照读+下发」或「delete 软删+下发」临界区。
    ///
    /// 死锁规避:同时锁多个节点(隧道规则 delete 跨多跳)时,**按 node_id 升序**获取——
    /// 任意两个并发的多节点持锁者获取顺序一致,不构成环路等待。reconcile 永远只锁单节点
    /// (单 Agent 订阅),单次获取不可能死锁。入参先去重排序。
    pub async fn lock_nodes(&self, node_ids: &[i64]) -> Vec<OwnedMutexGuard<()>> {
        let mut ids: Vec<i64> = node_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        // 先在 std 锁内拿/建每个 node 的 Arc<AsyncMutex>(瞬时,无 await),再逐个异步 lock。
        let mutexes: Vec<Arc<AsyncMutex<()>>> = {
            let mut reg = self.node_locks.lock().unwrap();
            ids.iter()
                .map(|id| reg.entry(*id).or_default().clone())
                .collect()
        };
        let mut guards = Vec::with_capacity(mutexes.len());
        for m in mutexes {
            guards.push(m.lock_owned().await);
        }
        guards
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
        let (mut rx, _tx, gen) = d.subscribe(1);
        assert!(d.dispatch(1, dummy_cmd()), "dispatch 应送到");
        assert!(rx.recv().await.is_some());
        d.unsubscribe_if(1, gen);
        assert!(!d.dispatch(1, dummy_cmd()), "unsubscribe 后不应再送到");
    }

    #[tokio::test]
    async fn unsubscribe_if_does_not_kill_newer_generation() {
        let d = CommandDispatcher::new();
        let (_rx_old, _tx_old, gen_old) = d.subscribe(7);
        let (mut rx_new, _tx_new, _gen_new) = d.subscribe(7); // 替换旧 entry
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

    #[tokio::test]
    async fn dispatcher_normal_send_ok() {
        // 未满时:dispatch 正常入队,agent 侧 receiver 可收到。
        let d = CommandDispatcher::new();
        let (mut rx, _tx, _gen) = d.subscribe(3);
        assert!(d.dispatch(3, dummy_cmd()), "未满应入队成功");
        assert!(rx.recv().await.is_some(), "agent 侧应收到");
    }

    #[tokio::test]
    async fn dispatcher_bounded_rejects_when_full() {
        // 不消费 receiver,持续 dispatch 填满有界 channel(容量 CHANNEL_CAPACITY)。
        // 满后再 dispatch 必须返回 false(背压拒绝),而非 panic / 内存无限增长。
        let d = CommandDispatcher::new();
        let (_rx, _tx, _gen) = d.subscribe(5); // 持有 rx 但不 recv,使队列填满后保持满
        for i in 0..CHANNEL_CAPACITY {
            assert!(
                d.dispatch(5, dummy_cmd()),
                "容量内第 {i} 条应入队成功"
            );
        }
        // 第 CHANNEL_CAPACITY+1 条:channel 已满,try_send 失败 → false。
        assert!(
            !d.dispatch(5, dummy_cmd()),
            "channel 满后 dispatch 应返回 false(不阻塞、不无限堆积)"
        );
    }

    #[tokio::test]
    async fn reconcile_replay_via_sender_is_lossless_over_capacity() {
        // 回归:reconcile 重放走 subscribe 返回的 sender + send().await,即使重放命令数
        // 超过 channel 容量也**不丢**(消费者在 drain)。验证 B1 修复:有界 channel 不再
        // 静默截断超额 reconcile 批次(末尾权威 ReconcileRules 必须到达)。
        let d = CommandDispatcher::new();
        let (mut rx, tx, _gen) = d.subscribe(9);
        let total = CHANNEL_CAPACITY + 50; // 故意超容量
        let producer = tokio::spawn(async move {
            for _ in 0..total {
                // send().await 满则等待消费者拉取,不丢弃。
                tx.send(dummy_cmd()).await.expect("消费者在,send 不应失败");
            }
        });
        let mut received = 0usize;
        while received < total {
            assert!(rx.recv().await.is_some(), "应能收齐全部重放命令");
            received += 1;
        }
        producer.await.unwrap();
        assert_eq!(received, total, "超容量重放必须无丢失全部送达");
    }
}
