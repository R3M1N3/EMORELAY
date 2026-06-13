//! 把一条关联隧道的业务规则按 hop 拆成 N 个带 TunnelContext 的 proto Rule(P3b)。
//! entry 监听业务 listen_port + dial 下一跳;mid 监听 self_inter_port + dial 下一跳;
//! exit 监听 self_inter_port + connect 业务 target。限速只在 entry 计(mid/exit bandwidth_mbps=0)。
//! 纯函数,便于单测;dispatch 接入留数据面。
use emorelay_common::control::v1::{Rule as ProtoRule, TunnelContext, TunnelRole};

/// 拆分输入:业务规则字段 + 隧道 id/transport。
pub struct SplitInput {
    pub rule_id: i64,
    pub protocol: String,
    pub listen_ip: String,
    pub listen_port: u32,
    pub target_host: String,
    pub target_port: u32,
    pub enabled: bool,
    pub bandwidth_mbps: i64,
    /// 并发连接上限。0 = 不限;仅 entry 生效。
    pub max_connections: i64,
    pub tunnel_id: i64,
    pub transport: String,
}

/// 单跳输入:节点 id + 该跳监听端口(entry 为 None)+ 节点可达地址。
pub struct HopInput {
    pub node_id: i64,
    pub inter_port: Option<i64>,
    pub addr: String,
}

/// 返回 (node_id, 该节点上要跑的 proto Rule)。hops 按 ordinal 升序。
pub fn split_tunnel_rule(input: &SplitInput, hops: &[HopInput]) -> Vec<(i64, ProtoRule)> {
    let n = hops.len();
    hops.iter().enumerate().map(|(i, hop)| {
        let role = if i == 0 {
            TunnelRole::Entry
        } else if i == n - 1 {
            TunnelRole::Exit
        } else {
            TunnelRole::Mid
        };
        let next = hops.get(i + 1);
        let tunnel = TunnelContext {
            tunnel_id: input.tunnel_id,
            role: role as i32,
            next_hop_addr: next.map(|h| h.addr.clone()).unwrap_or_default(),
            next_hop_inter_port: next.and_then(|h| h.inter_port).unwrap_or(0) as u32,
            self_inter_port: hop.inter_port.unwrap_or(0) as u32,
            transport: input.transport.clone(),
            self_ordinal: i as u32,
        };
        let proto = ProtoRule {
            id: input.rule_id,
            protocol: input.protocol.clone(),
            listen_ip: input.listen_ip.clone(),
            listen_port: input.listen_port,
            target_host: input.target_host.clone(),
            target_port: input.target_port,
            enabled: input.enabled,
            // 限速/连接数只在 entry 起作用,mid/exit 置 0 避免逐跳重复计。
            bandwidth_mbps: if i == 0 { input.bandwidth_mbps } else { 0 },
            tunnel: Some(tunnel),
            max_connections: if i == 0 { input.max_connections } else { 0 },
            // 隧道规则走隧道 transport,不做明文协议嗅探(嗅探仅普通 TCP relay)。
            blocked_protocols: 0,
        };
        (hop.node_id, proto)
    }).collect()
}
