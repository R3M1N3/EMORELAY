use crate::models::rule::Rule as DbRule;
use emorelay_common::control::v1::{
    command::Body, ApplyRule, Command, RemoveRule, Rule as ProtoRule, TargetEndpoint,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct ExtraTarget {
    host: String,
    port: u32,
}

/// 解析 forward_rules.extra_targets(JSON 数组)为 proto TargetEndpoint 列表。
/// 解析失败/空 → 空列表(降级为单目标,不致命)。
fn parse_extra_targets(json: Option<&str>) -> Vec<TargetEndpoint> {
    let Some(s) = json else { return Vec::new() };
    match serde_json::from_str::<Vec<ExtraTarget>>(s) {
        Ok(v) => v
            .into_iter()
            .map(|t| TargetEndpoint { host: t.host, port: t.port })
            .collect(),
        Err(e) => {
            // 写入侧 validate_targets 保证格式;此处损坏属异常,降级单目标并告警(不致命)。
            tracing::warn!(error = ?e, "corrupt extra_targets JSON; falling back to single target");
            Vec::new()
        }
    }
}

/// DB Rule → 协议 Rule。bandwidth_mbps 派生列 None→0(无限速)。
/// blocked_protocols 是节点级嗅探阻断位掩码(由调用方查节点设置传入,仅非隧道 TCP relay 用)。
pub fn rule_to_proto(rule: &DbRule, blocked_protocols: u32) -> ProtoRule {
    ProtoRule {
        id: rule.id,
        protocol: rule.protocol.clone(),
        listen_ip: rule.listen_ip.clone(),
        listen_port: rule.listen_port as u32,
        target_host: rule.target_host.clone(),
        target_port: rule.target_port as u32,
        enabled: rule.enabled != 0,
        bandwidth_mbps: rule.bandwidth_mbps.unwrap_or(0),
        tunnel: None,
        max_connections: rule.max_connections.unwrap_or(0),
        blocked_protocols,
        extra_targets: parse_extra_targets(rule.extra_targets.as_deref()),
        lb_strategy: rule.lb_strategy.clone(),
        send_proxy_protocol: rule.send_proxy_protocol != 0,
    }
}

/// SQLite `datetime('now')` 格式 "YYYY-MM-DD HH:MM:SS" → unix 秒（UTC）。
/// 失败返回 0。
/// pub:登录到期检查与 user_quota sweeper 复用,避免多处独立解析漂移。
pub fn parse_sqlite_datetime(s: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S")
        .map(|n| n.and_utc().timestamp())
        .unwrap_or(0)
}

pub fn apply_command(rule: &DbRule, blocked_protocols: u32) -> Command {
    Command {
        body: Some(Body::ApplyRule(ApplyRule {
            rule: Some(rule_to_proto(rule, blocked_protocols)),
        })),
    }
}

pub fn remove_command(rule_id: i64) -> Command {
    Command {
        body: Some(Body::RemoveRule(RemoveRule { rule_id })),
    }
}

pub fn restart_command(rule_id: i64) -> Command {
    Command {
        body: Some(Body::RestartRule(
            emorelay_common::control::v1::RestartRule { rule_id },
        )),
    }
}
