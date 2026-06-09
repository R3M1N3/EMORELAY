use crate::models::rule::Rule as DbRule;
use emorelay_common::control::v1::{
    command::Body, ApplyRule, Command, RemoveRule, Rule as ProtoRule,
};

/// DB Rule → 协议 Rule。limit 字段 None→0；expires_at 解析 SQLite datetime('now') 格式。
pub fn rule_to_proto(rule: &DbRule) -> ProtoRule {
    ProtoRule {
        id: rule.id,
        protocol: rule.protocol.clone(),
        listen_ip: rule.listen_ip.clone(),
        listen_port: rule.listen_port as u32,
        target_host: rule.target_host.clone(),
        target_port: rule.target_port as u32,
        enabled: rule.enabled != 0,
        traffic_limit_bytes: rule.traffic_limit_bytes.unwrap_or(0),
        bandwidth_limit_mbps: rule.bandwidth_limit_mbps.unwrap_or(0),
        expires_at_unix: rule
            .expires_at
            .as_deref()
            .map(parse_sqlite_datetime)
            .unwrap_or(0),
    }
}

/// SQLite `datetime('now')` 格式 "YYYY-MM-DD HH:MM:SS" → unix 秒（UTC）。
/// 失败返回 0（不影响主流程：Agent 端 0 = 永不过期，等同于未启用）。
/// pub:auto_stop sweeper / service 也复用,避免两处独立解析漂移。
pub fn parse_sqlite_datetime(s: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S")
        .map(|n| n.and_utc().timestamp())
        .unwrap_or(0)
}

pub fn apply_command(rule: &DbRule) -> Command {
    Command {
        body: Some(Body::ApplyRule(ApplyRule {
            rule: Some(rule_to_proto(rule)),
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
