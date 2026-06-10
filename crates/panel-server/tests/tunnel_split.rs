use emorelay_common::control::v1::TunnelRole;
use panel_server::grpc::tunnel_split::{split_tunnel_rule, SplitInput, HopInput};

fn rule_input() -> SplitInput {
    SplitInput {
        rule_id: 100,
        protocol: "tcp".into(),
        listen_ip: "0.0.0.0".into(),
        listen_port: 20000,
        target_host: "9.9.9.9".into(),
        target_port: 443,
        enabled: true,
        bandwidth_mbps: 50,
        tunnel_id: 7,
        transport: "tls".into(),
    }
}

#[test]
fn two_hop_split_entry_and_exit() {
    let hops = vec![
        HopInput { node_id: 1, inter_port: None,        addr: "10.0.0.1".into() },
        HopInput { node_id: 2, inter_port: Some(30001), addr: "10.0.0.2".into() },
    ];
    let out = split_tunnel_rule(&rule_input(), &hops);
    assert_eq!(out.len(), 2);

    let (n0, r0) = &out[0];
    assert_eq!(*n0, 1);
    let t0 = r0.tunnel.as_ref().unwrap();
    assert_eq!(t0.role, TunnelRole::Entry as i32);
    assert_eq!(t0.next_hop_addr, "10.0.0.2");
    assert_eq!(t0.next_hop_inter_port, 30001);
    assert_eq!(t0.self_inter_port, 0);
    assert_eq!(t0.transport, "tls");
    assert_eq!(r0.listen_port, 20000);
    assert_eq!(r0.bandwidth_mbps, 50);

    let (n1, r1) = &out[1];
    assert_eq!(*n1, 2);
    let t1 = r1.tunnel.as_ref().unwrap();
    assert_eq!(t1.role, TunnelRole::Exit as i32);
    assert_eq!(t1.self_inter_port, 30001);
    assert_eq!(t1.next_hop_inter_port, 0);
    assert_eq!(r1.target_host, "9.9.9.9");
    assert_eq!(r1.bandwidth_mbps, 0);
}

#[test]
fn three_hop_split_has_mid() {
    let hops = vec![
        HopInput { node_id: 1, inter_port: None,        addr: "10.0.0.1".into() },
        HopInput { node_id: 2, inter_port: Some(30001), addr: "10.0.0.2".into() },
        HopInput { node_id: 3, inter_port: Some(30002), addr: "10.0.0.3".into() },
    ];
    let out = split_tunnel_rule(&rule_input(), &hops);
    assert_eq!(out.len(), 3);
    let t_mid = out[1].1.tunnel.as_ref().unwrap();
    assert_eq!(t_mid.role, TunnelRole::Mid as i32);
    assert_eq!(t_mid.self_inter_port, 30001);
    assert_eq!(t_mid.next_hop_addr, "10.0.0.3");
    assert_eq!(t_mid.next_hop_inter_port, 30002);
    assert_eq!(out[1].1.bandwidth_mbps, 0);
    assert_eq!(out[2].1.tunnel.as_ref().unwrap().role, TunnelRole::Exit as i32);
}
