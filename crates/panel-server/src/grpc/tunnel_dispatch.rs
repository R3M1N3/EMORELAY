//! 隧道规则真实下发(P3b 数据面)。关联隧道的规则用 split_tunnel_rule 拆成
//! per-hop Rule 分发到链上每个节点;非隧道规则保持原单节点路径。tls/wss 隧道
//! 的 hop 凭据由内置 CA 即时签发(不入 DB,重签幂等),创建/restart/reconcile 下发。
//! Agent 离线时 dispatch 返回 false 仅 warn——reconcile 在下次 subscribe 时兜底。
use emorelay_common::control::v1::{
    command::Body, ApplyRule, Command, RevokeTunnelCredentials, Rule as ProtoRule,
    TunnelCredentials,
};
use tracing::warn;

use crate::grpc::commands::{apply_command, remove_command, restart_command};
use crate::grpc::tunnel_split::{split_tunnel_rule, HopInput, SplitInput};
use crate::models::rule::Rule as DbRule;
use crate::models::tunnel::{Tunnel, TunnelHop};
use crate::state::AppState;

/// 把关联隧道的 DB 规则拆成 (node_id, proto Rule) 列表。
/// 隧道已删/无 hop → Ok(None);dispatch_rule_apply 对此 fail-closed(不下发,绝不退化成明文直连)。正常流程不可达(删除保护拦截)。
async fn split_for(
    state: &AppState,
    rule: &DbRule,
    tunnel_id: i64,
) -> sqlx::Result<Option<Vec<(i64, ProtoRule)>>> {
    let Some(tunnel) = Tunnel::find_by_id(&state.pool, tunnel_id).await? else {
        return Ok(None);
    };
    let hops = TunnelHop::list_for_tunnel(&state.pool, tunnel_id).await?;
    if hops.is_empty() {
        return Ok(None);
    }
    let mut hop_inputs = Vec::with_capacity(hops.len());
    for h in &hops {
        let addr: Option<(String,)> =
            sqlx::query_as("SELECT public_ip FROM nodes WHERE id = ? AND deleted_at IS NULL")
                .bind(h.node_id)
                .fetch_optional(&state.pool)
                .await?;
        hop_inputs.push(HopInput {
            node_id: h.node_id,
            inter_port: h.inter_port,
            addr: addr.map(|a| a.0).unwrap_or_default(),
        });
    }
    let input = SplitInput {
        rule_id: rule.id,
        protocol: rule.protocol.clone(),
        listen_ip: rule.listen_ip.clone(),
        listen_port: rule.listen_port as u32,
        target_host: rule.target_host.clone(),
        target_port: rule.target_port as u32,
        enabled: rule.enabled != 0,
        bandwidth_mbps: rule.bandwidth_mbps.unwrap_or(0),
        max_connections: rule.max_connections.unwrap_or(0),
        tunnel_id,
        transport: tunnel.transport.clone(),
    };
    Ok(Some(split_tunnel_rule(&input, &hop_inputs)))
}

fn warn_offline(node_id: i64, rule_id: i64, what: &str) {
    warn!(node_id, rule_id, "agent offline; {what} will sync at next register");
}

/// apply(create/update/enable/disable/限速变更统一入口)。
pub async fn dispatch_rule_apply(state: &AppState, rule: &DbRule) -> sqlx::Result<()> {
    match rule.tunnel_id {
        Some(tid) => {
            if let Some(parts) = split_for(state, rule, tid).await? {
                for (node_id, proto) in parts {
                    let cmd = Command {
                        body: Some(Body::ApplyRule(ApplyRule { rule: Some(proto) })),
                    };
                    if !state.dispatcher.dispatch(node_id, cmd) {
                        warn_offline(node_id, rule.id, "tunnel hop rule");
                    }
                }
                return Ok(());
            }
            // fail-closed:隧道不可见(理论不可达,删除保护拦截)时**不下发**——
            // 绝不让本应走加密隧道的规则退化成 entry 节点明文直连。
            warn!(rule_id = rule.id, tunnel_id = tid, "tunnel missing for rule; apply NOT dispatched");
            Ok(())
        }
        None => {
            if !state.dispatcher.dispatch(rule.node_id, apply_command(rule)) {
                warn_offline(rule.node_id, rule.id, "rule");
            }
            Ok(())
        }
    }
}

async fn tunnel_node_ids(state: &AppState, tunnel_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar("SELECT node_id FROM tunnel_hops WHERE tunnel_id = ? ORDER BY ordinal")
        .bind(tunnel_id)
        .fetch_all(&state.pool)
        .await
}

/// remove:隧道规则对链上每个节点发 RemoveRule;非隧道单节点。
pub async fn dispatch_rule_remove(state: &AppState, rule: &DbRule) -> sqlx::Result<()> {
    let nodes = match rule.tunnel_id {
        Some(tid) => tunnel_node_ids(state, tid).await?,
        None => vec![rule.node_id],
    };
    for node_id in nodes {
        if !state.dispatcher.dispatch(node_id, remove_command(rule.id)) {
            warn_offline(node_id, rule.id, "rule removal");
        }
    }
    Ok(())
}

/// restart。返回是否至少送达一个节点(rules.rs restart 响应里回显)。
pub async fn dispatch_rule_restart(state: &AppState, rule: &DbRule) -> sqlx::Result<bool> {
    let nodes = match rule.tunnel_id {
        Some(tid) => tunnel_node_ids(state, tid).await?,
        None => vec![rule.node_id],
    };
    let mut any = false;
    for node_id in nodes {
        any |= state.dispatcher.dispatch(node_id, restart_command(rule.id));
    }
    Ok(any)
}

fn credentials_command(state: &AppState, tunnel_id: i64, ordinal: i64) -> Option<Command> {
    match crate::tls::issue::issue_tunnel_hop_certs(&state.ca, tunnel_id, ordinal) {
        Ok(c) => Some(Command {
            body: Some(Body::TunnelCredentials(TunnelCredentials {
                tunnel_id,
                ordinal: ordinal as i32,
                server_cert_pem: c.server_cert_pem,
                server_key_pem: c.server_key_pem,
                client_cert_pem: c.client_cert_pem,
                client_key_pem: c.client_key_pem,
                ca_pem: state.ca.ca_pem.clone(),
            })),
        }),
        Err(e) => {
            warn!(error = ?e, tunnel_id, ordinal, "issue tunnel hop certs failed");
            None
        }
    }
}

/// tls/wss 隧道:为每个 hop 即时签发凭据并下发。tcp 隧道 no-op。
pub async fn dispatch_tunnel_credentials(state: &AppState, tunnel: &Tunnel) -> sqlx::Result<()> {
    if tunnel.transport == "tcp" {
        return Ok(());
    }
    for h in TunnelHop::list_for_tunnel(&state.pool, tunnel.id).await? {
        if let Some(cmd) = credentials_command(state, tunnel.id, h.ordinal) {
            if !state.dispatcher.dispatch(h.node_id, cmd) {
                warn!(node_id = h.node_id, tunnel_id = tunnel.id, "agent offline; credentials will resend at next register");
            }
        }
    }
    Ok(())
}

/// 删隧道后通知各 hop 清理凭据目录。
pub fn dispatch_revoke_tunnel_credentials(
    state: &AppState,
    tunnel_id: i64,
    hop_node_ids: &[i64],
) {
    for node_id in hop_node_ids {
        let cmd = Command {
            body: Some(Body::RevokeTunnelCredentials(RevokeTunnelCredentials { tunnel_id })),
        };
        let _ = state.dispatcher.dispatch(*node_id, cmd);
    }
}

/// reconcile:Agent 重连后重放该节点应有的全部命令(顺序敏感:凭据先于隧道规则)。
/// 1) 本节点的非隧道规则;2) 本节点参与的每个活跃隧道:凭据(tls/wss) → 该隧道
/// 全部活跃规则 split 后取本节点份额(entry/mid/exit 均覆盖——隧道规则行的 node_id
/// 是 entry,mid/exit 节点上没有 forward_rules 行,只能从 tunnel_hops 反查)。
pub async fn reconcile_commands_for_node(
    state: &AppState,
    node_id: i64,
) -> sqlx::Result<Vec<Command>> {
    let mut out = Vec::new();
    for rule in DbRule::list_active_for_node(&state.pool, node_id).await? {
        if rule.tunnel_id.is_none() {
            out.push(apply_command(&rule));
        }
    }
    for tid in TunnelHop::list_tunnel_ids_for_node(&state.pool, node_id).await? {
        let Some(tunnel) = Tunnel::find_by_id(&state.pool, tid).await? else {
            continue;
        };
        if tunnel.transport != "tcp" {
            if let Some(hop) = TunnelHop::find_for_node(&state.pool, tid, node_id).await? {
                if let Some(cmd) = credentials_command(state, tid, hop.ordinal) {
                    out.push(cmd);
                }
            }
        }
        for rule in DbRule::list_active_for_tunnel(&state.pool, tid).await? {
            if let Some(parts) = split_for(state, &rule, tid).await? {
                for (nid, proto) in parts {
                    if nid == node_id {
                        out.push(Command {
                            body: Some(Body::ApplyRule(ApplyRule { rule: Some(proto) })),
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}
