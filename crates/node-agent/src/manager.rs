use anyhow::Result;
use emorelay_common::control::v1::Rule;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use crate::relay::tcp::{self, TcpRelayHandle};
use crate::relay::udp::{self, UdpRelayHandle};
use crate::stats::StatsCollector;

/// 一条规则可能同时持有 TCP 和 UDP relay（protocol = tcp_udp）。
#[derive(Default)]
struct RuleHandles {
    rule: Option<Rule>,
    tcp: Option<TcpRelayHandle>,
    udp: Option<UdpRelayHandle>,
}

impl RuleHandles {
    async fn stop_all(mut self) {
        if let Some(h) = self.tcp.take() {
            h.stop().await;
        }
        if let Some(h) = self.udp.take() {
            h.stop().await;
        }
    }
}

/// 节点本地规则状态。apply / remove / restart 是幂等的：apply 同 id 时旧 task 先 stop。
pub struct RuleManager {
    handles: HashMap<i64, RuleHandles>,
    stats: Arc<StatsCollector>,
}

impl RuleManager {
    pub fn new(stats: Arc<StatsCollector>) -> Self {
        Self {
            handles: HashMap::new(),
            stats,
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
        let mut bundle = RuleHandles {
            rule: Some(rule.clone()),
            ..Default::default()
        };
        match rule.protocol.as_str() {
            "tcp" => {
                bundle.tcp = Some(tcp::start(rule.clone(), self.stats.clone()).await?);
            }
            "udp" => {
                bundle.udp = Some(udp::start(rule.clone(), self.stats.clone()).await?);
            }
            "tcp_udp" => {
                bundle.tcp = Some(tcp::start(rule.clone(), self.stats.clone()).await?);
                // 若 UDP start 失败，必须主动 stop 已启动的 TCP，否则 listener task 泄漏。
                match udp::start(rule.clone(), self.stats.clone()).await {
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
