mod config;
mod control;
mod limit;
mod manager;
mod relay;
mod stats;
mod store;
mod tunnel;
mod system;

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::control::ControlClient;
use crate::manager::RuleManager;
use crate::stats::StatsCollector;
use crate::store::ConfigStore;
use crate::system::SystemSampler;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// 统计上报间隔（秒）。env `AGENT_STATS_INTERVAL_SECS` 覆盖；默认 60s。
/// 测试时可设小值（如 5）快速观察上报；生产保持默认。
fn stats_interval() -> Duration {
    let secs: u64 = std::env::var("AGENT_STATS_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    Duration::from_secs(secs.max(1))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    info!(
        node_id = config.node_id,
        endpoint = %config.control_endpoint,
        state_path = %config.state_path,
        "node-agent starting"
    );

    let stats = Arc::new(StatsCollector::new());
    let manager = Arc::new(Mutex::new(RuleManager::new(stats.clone())));
    let store = Arc::new(ConfigStore::new(config.state_path.clone()));
    // 跨会话保留:重连不会重置 CPU/MEM 采样基线或网卡字节累计,
    // 保证 60s bucket 不被人为撕裂。
    let sampler = Arc::new(SystemSampler::new());

    // 启动时立刻加载本地规则并 apply，让转发任务在 server 未连通前已经在跑
    // （plan.md 第四节"没有主控连接时保持已有规则继续运行"）。
    let persisted = match store.load().await {
        Ok(rs) => rs,
        Err(e) => {
            warn!(error = ?e, "load persisted state failed; starting fresh");
            Vec::new()
        }
    };
    {
        let mut mgr = manager.lock().await;
        for rule in persisted {
            let rule_id = rule.id;
            if let Err(e) = mgr.apply(rule).await {
                warn!(rule_id, error = ?e, "apply persisted rule failed");
            }
        }
    }

    // 主循环：会话异常退出后等待 RETRY_BACKOFF 重连。
    // manager 跨会话保留：listener 任务在 connect 失败期间也持续转发。
    loop {
        match run_session(
            &config,
            manager.clone(),
            store.clone(),
            stats.clone(),
            sampler.clone(),
        )
        .await
        {
            Ok(()) => warn!("session ended cleanly; reconnecting after backoff"),
            Err(e) => error!(error = ?e, "session error; reconnecting after backoff"),
        }
        tokio::time::sleep(RETRY_BACKOFF).await;
    }
}

async fn run_session(
    config: &Config,
    manager: Arc<Mutex<RuleManager>>,
    store: Arc<ConfigStore>,
    stats: Arc<StatsCollector>,
    sampler: Arc<SystemSampler>,
) -> Result<()> {
    let mut client = ControlClient::connect(
        config.control_endpoint.clone(),
        config.node_id,
        config.token.clone(),
        config.grpc_ca_cert.clone(),
        config.grpc_client_cert.clone(),
        config.grpc_client_key.clone(),
    )
    .await?;
    client.register().await?;

    let mut command_stream = client.subscribe_commands().await?;

    let mut hb_tick = interval(HEARTBEAT_INTERVAL);
    hb_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    hb_tick.tick().await;

    let mut stats_tick = interval(stats_interval());
    stats_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    stats_tick.tick().await;

    loop {
        tokio::select! {
            msg = command_stream.message() => {
                match msg {
                    Ok(Some(cmd)) => {
                        if let Err(e) = handle_command(&manager, &store, cmd).await {
                            error!(error = ?e, "command apply failed");
                        }
                    }
                    Ok(None) => {
                        info!("command stream closed by server");
                        return Ok(());
                    }
                    Err(status) => {
                        error!(?status, "command stream error");
                        anyhow::bail!("command stream: {status}");
                    }
                }
            }
            _ = hb_tick.tick() => {
                let m = sampler.refresh_metrics();
                client.heartbeat(m.cpu_usage, m.memory_usage, m.load_average).await?;
                info!(
                    cpu = m.cpu_usage,
                    mem = m.memory_usage,
                    load = m.load_average,
                    "heartbeat sent"
                );
            }
            _ = stats_tick.tick() => {
                // 节点级 stats 必须先报：drain 同时刷出网卡 rx/tx 增量,
                // 与本 bucket 内 rule_stats 的累计逻辑对齐(同一个分钟窗口)。
                if let Err(e) = report_node_stats(config, &mut client, &sampler).await {
                    warn!(error = ?e, "report_node_stats failed");
                }
                if let Err(e) = report_stats(config, &mut client, &stats).await {
                    warn!(error = ?e, "report_stats failed");
                }
            }
        }
    }
}

async fn report_node_stats(
    config: &Config,
    client: &mut ControlClient,
    sampler: &SystemSampler,
) -> Result<()> {
    use emorelay_common::control::v1::{NodeStatsBatch, NodeStatsBucket};

    let sample = sampler.drain();

    let now_unix = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => {
            warn!(error = ?e, "system clock before UNIX_EPOCH; skip node stats");
            return Ok(());
        }
    };
    let bucket_at_unix = (now_unix / 60) * 60;

    let bucket = NodeStatsBucket {
        bucket_at_unix,
        cpu_usage: sample.cpu_usage,
        memory_usage: sample.memory_usage,
        load_average: sample.load_average,
        rx_bytes: sample.rx_bytes_delta,
        tx_bytes: sample.tx_bytes_delta,
    };
    let batch = NodeStatsBatch {
        node_id: config.node_id,
        buckets: vec![bucket],
    };

    client
        .report_node_stats(tokio_stream::iter(vec![batch]))
        .await?;
    info!(
        cpu = sample.cpu_usage,
        mem = sample.memory_usage,
        rx = sample.rx_bytes_delta,
        tx = sample.tx_bytes_delta,
        "node stats reported"
    );
    Ok(())
}

async fn report_stats(
    config: &Config,
    client: &mut ControlClient,
    stats: &StatsCollector,
) -> Result<()> {
    use emorelay_common::control::v1::{RuleStatsBatch, RuleStatsBucket};

    let snapshot = stats.drain_snapshot();
    if snapshot.is_empty() {
        return Ok(());
    }
    // 把窗口边界对齐到分钟，方便 server UPSERT 同窗口累加。
    // 系统时钟异常（在 UNIX_EPOCH 之前）则跳过本次上报，避免所有 bucket 撞到 1970-01-01。
    let now_unix = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => {
            warn!(error = ?e, "system clock before UNIX_EPOCH; skip stats report");
            return Ok(());
        }
    };
    let bucket_at_unix = (now_unix / 60) * 60;

    let buckets: Vec<RuleStatsBucket> = snapshot
        .iter()
        .map(|s| RuleStatsBucket {
            rule_id: s.rule_id,
            bucket_at_unix,
            rx_bytes: s.rx_bytes,
            tx_bytes: s.tx_bytes,
            connection_count: s.connection_count,
            error_count: s.error_count,
        })
        .collect();
    let batch = RuleStatsBatch {
        node_id: config.node_id,
        buckets,
    };
    info!(rules = snapshot.len(), "reporting rule stats");
    client
        .report_rule_stats(tokio_stream::iter(vec![batch]))
        .await?;
    Ok(())
}

async fn handle_command(
    manager: &Mutex<RuleManager>,
    store: &ConfigStore,
    cmd: emorelay_common::control::v1::Command,
) -> Result<()> {
    use emorelay_common::control::v1::command::Body;
    let Some(body) = cmd.body else {
        return Ok(());
    };

    // 锁内执行 apply / remove / restart，然后立即在锁内取快照；锁外做磁盘 IO。
    let snapshot = {
        let mut mgr = manager.lock().await;
        match body {
            Body::ApplyRule(apply) => {
                if let Some(rule) = apply.rule {
                    info!(rule_id = rule.id, enabled = rule.enabled, "apply rule");
                    mgr.apply(rule).await?;
                }
            }
            Body::RemoveRule(remove) => {
                info!(rule_id = remove.rule_id, "remove rule");
                mgr.remove(remove.rule_id).await;
            }
            Body::EnableRule(_) | Body::DisableRule(_) => {
                // server 在 enable/disable 时同步推 ApplyRule（含新 enabled），这两个变体本地无须独立处理。
            }
            Body::RestartRule(r) => {
                info!(rule_id = r.rule_id, "restart rule");
                mgr.restart(r.rule_id).await?;
            }
            Body::TunnelCredentials(c) => {
                // P3b 数据面尚未落地隧道凭据处理(后续任务);先确认收到,不改规则状态。
                info!(
                    tunnel_id = c.tunnel_id,
                    ordinal = c.ordinal,
                    "tunnel credentials received (data plane pending)"
                );
            }
            Body::RevokeTunnelCredentials(c) => {
                info!(
                    tunnel_id = c.tunnel_id,
                    "tunnel credentials revoke received (data plane pending)"
                );
            }
        }
        mgr.current_rules()
    };
    if let Err(e) = store.save(&snapshot).await {
        warn!(error = ?e, "persist rule state failed");
    }
    Ok(())
}
