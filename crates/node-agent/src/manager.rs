use anyhow::Result;
use emorelay_common::control::v1::Rule;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use crate::relay::tcp::{self, TcpRelayHandle};
use crate::relay::udp::{self, UdpRelayHandle};
use crate::stats::StatsCollector;
use crate::tunnel::task::TunnelTaskHandle;

/// 一条规则可能同时持有 TCP 和 UDP relay（protocol = tcp_udp）。
#[derive(Default)]
struct RuleHandles {
    rule: Option<Rule>,
    tcp: Option<TcpRelayHandle>,
    udp: Option<UdpRelayHandle>,
    tunnel: Option<TunnelTaskHandle>,
}

impl RuleHandles {
    async fn stop_all(mut self) {
        if let Some(h) = self.tcp.take() {
            h.stop().await;
        }
        if let Some(h) = self.udp.take() {
            h.stop().await;
        }
        if let Some(h) = self.tunnel.take() {
            h.stop().await;
        }
    }
}

/// 节点本地规则状态。apply / remove / restart 是幂等的：apply 同 id 时旧 task 先 stop。
pub struct RuleManager {
    handles: HashMap<i64, RuleHandles>,
    stats: Arc<StatsCollector>,
    /// 隧道凭据根目录(AGENT_DATA_DIR);tls/wss transport 从这里读证书。
    data_dir: String,
}

impl RuleManager {
    pub fn new(stats: Arc<StatsCollector>, data_dir: String) -> Self {
        Self {
            handles: HashMap::new(),
            stats,
            data_dir,
        }
    }

    /// 应用规则。enabled=false 视作 "存在但不监听"（手动停掉对应 task）。
    /// protocol=tcp_udp 时同时启动 TCP 与 UDP 两个 listener。
    pub async fn apply(&mut self, rule: Rule) -> Result<()> {
        if let Some(old) = self.handles.remove(&rule.id) {
            old.stop_all().await;
        }
        if !rule.enabled {
            info!(rule_id = rule.id, "rule disabled; no listener");
            return Ok(());
        }
        // P3b:带 tunnel 上下文 → TunnelTask(entry/mid/exit),不走普通 relay。
        if let Some(ctx) = rule.tunnel.as_ref() {
            // split 已保证仅 entry 的 bandwidth_mbps 非 0,mid/exit 自然拿 None。
            let bucket = crate::limit::TokenBucket::from_mbps(rule.bandwidth_mbps);
            let transport = crate::tunnel::make_transport(ctx, &self.data_dir)?;
            let handle =
                crate::tunnel::task::start(rule.clone(), self.stats.clone(), bucket, transport)
                    .await?;
            self.handles.insert(
                rule.id,
                RuleHandles {
                    rule: Some(rule),
                    tunnel: Some(handle),
                    ..Default::default()
                },
            );
            return Ok(());
        }
        let mut bundle = RuleHandles {
            rule: Some(rule.clone()),
            ..Default::default()
        };
        // P2 限速:per-rule 桶;tcp_udp 两个 listener 共享同一实例(rx+tx 合并计)。
        let bucket = crate::limit::TokenBucket::from_mbps(rule.bandwidth_mbps);
        match rule.protocol.as_str() {
            "tcp" => {
                bundle.tcp =
                    Some(tcp::start(rule.clone(), self.stats.clone(), bucket.clone()).await?);
            }
            "udp" => {
                bundle.udp =
                    Some(udp::start(rule.clone(), self.stats.clone(), bucket.clone()).await?);
            }
            "tcp_udp" => {
                bundle.tcp =
                    Some(tcp::start(rule.clone(), self.stats.clone(), bucket.clone()).await?);
                // 若 UDP start 失败，必须主动 stop 已启动的 TCP，否则 listener task 泄漏。
                match udp::start(rule.clone(), self.stats.clone(), bucket.clone()).await {
                    Ok(h) => bundle.udp = Some(h),
                    Err(e) => {
                        if let Some(h) = bundle.tcp.take() {
                            h.stop().await;
                        }
                        return Err(e);
                    }
                }
            }
            other => {
                warn!(rule_id = rule.id, protocol = %other, "unknown protocol; skip");
                return Ok(());
            }
        }
        self.handles.insert(rule.id, bundle);
        Ok(())
    }

    pub async fn remove(&mut self, rule_id: i64) {
        if let Some(h) = self.handles.remove(&rule_id) {
            h.stop_all().await;
        }
    }

    /// 配置对账:删除本地任何不在 keep_ids 内的规则(断网期间被删的孤儿)。
    /// 返回被删的 rule id 列表(供日志/审计)。在 reconcile ApplyRule 重放之后调用。
    pub async fn reconcile(&mut self, keep_ids: &[i64]) -> Vec<i64> {
        let keep: std::collections::HashSet<i64> = keep_ids.iter().copied().collect();
        let orphans: Vec<i64> = self
            .handles
            .keys()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in &orphans {
            if let Some(h) = self.handles.remove(id) {
                h.stop_all().await;
            }
        }
        orphans
    }

    pub async fn restart(&mut self, rule_id: i64) -> Result<()> {
        if let Some(bundle) = self.handles.remove(&rule_id) {
            let rule = bundle.rule.clone();
            bundle.stop_all().await;
            if let Some(rule) = rule {
                self.apply(rule).await?;
            }
        }
        Ok(())
    }

    /// 当前已加载的规则快照（用于落盘 / 上报）。
    pub fn current_rules(&self) -> Vec<Rule> {
        self.handles
            .values()
            .filter_map(|h| h.rule.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::{TunnelContext, TunnelRole};
    use std::net::TcpListener as StdTcpListener;
    use std::time::Duration;
    use tokio::net::TcpListener;

    fn ephemeral_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn entry_tunnel_rule(listen_port: u16) -> Rule {
        Rule {
            id: 77,
            protocol: "tcp".into(),
            listen_ip: "127.0.0.1".into(),
            listen_port: listen_port as u32,
            target_host: "127.0.0.1".into(),
            target_port: 1,
            enabled: true,
            bandwidth_mbps: 0,
            max_connections: 0,
            tunnel: Some(TunnelContext {
                tunnel_id: 3,
                role: TunnelRole::Entry as i32,
                next_hop_addr: "127.0.0.1".into(),
                next_hop_inter_port: 1,
                self_inter_port: 0,
                transport: "tcp".into(),
                self_ordinal: 0,
            }),
        }
    }

    /// 带 tunnel 的 Rule 走 TunnelTask:apply 占用 listen 端口,remove 释放。
    #[tokio::test]
    async fn apply_tunnel_rule_starts_task_and_remove_releases_port() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused-data-dir".into());
        let port = ephemeral_port();

        mgr.apply(entry_tunnel_rule(port)).await.expect("apply tunnel rule");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(mgr.current_rules().len(), 1);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).await.is_err(),
            "entry 应占用业务端口"
        );

        mgr.remove(77).await;
        TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("remove 后端口应释放");
    }

    /// 未知 transport(tls 未落地阶段同理)→ apply 报错,不留半启动状态。
    #[tokio::test]
    async fn apply_tunnel_rule_with_unknown_transport_errors() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused".into());
        let mut rule = entry_tunnel_rule(ephemeral_port());
        rule.tunnel.as_mut().unwrap().transport = "quic".into();
        assert!(mgr.apply(rule).await.is_err());
        assert!(mgr.current_rules().is_empty());
    }

    fn plain_rule(id: i64, listen_port: u16) -> Rule {
        Rule {
            id,
            protocol: "tcp".into(),
            listen_ip: "127.0.0.1".into(),
            listen_port: listen_port as u32,
            target_host: "127.0.0.1".into(),
            target_port: 1,
            enabled: true,
            bandwidth_mbps: 0,
            max_connections: 0,
            tunnel: None,
        }
    }

    /// 对账:删除不在 keep 集合内的孤儿,保留集合内的;返回被删 id。
    #[tokio::test]
    async fn reconcile_removes_orphans_and_keeps_authoritative() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused".into());
        let p1 = ephemeral_port();
        let p2 = ephemeral_port();
        mgr.apply(plain_rule(1, p1)).await.unwrap();
        mgr.apply(plain_rule(2, p2)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(mgr.current_rules().len(), 2);

        // 权威集合只含 1 → 规则 2 是孤儿被删,端口释放。
        let removed = mgr.reconcile(&[1]).await;
        assert_eq!(removed, vec![2]);
        let ids: Vec<i64> = mgr.current_rules().iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1]);
        TcpListener::bind(("127.0.0.1", p2))
            .await
            .expect("孤儿规则 2 端口应释放");
    }

    /// 空权威集合 → 清空全部本地规则(该节点不应运行任何规则)。
    #[tokio::test]
    async fn reconcile_empty_set_clears_all() {
        let stats = Arc::new(StatsCollector::new());
        let mut mgr = RuleManager::new(stats, "./unused".into());
        mgr.apply(plain_rule(1, ephemeral_port())).await.unwrap();
        let mut removed = mgr.reconcile(&[]).await;
        removed.sort_unstable();
        assert_eq!(removed, vec![1]);
        assert!(mgr.current_rules().is_empty());
    }
}
