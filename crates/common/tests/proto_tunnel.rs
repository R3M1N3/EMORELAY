//! 验证 P3b 隧道 proto 类型生成正确且可构造。
use emorelay_common::control::v1::{
    command::Body, Command, RevokeTunnelCredentials, Rule, TunnelContext, TunnelCredentials,
    TunnelRole,
};

#[test]
fn rule_carries_tunnel_context() {
    let r = Rule {
        id: 1,
        protocol: "tcp".into(),
        listen_ip: "0.0.0.0".into(),
        listen_port: 20000,
        target_host: "1.2.3.4".into(),
        target_port: 443,
        enabled: true,
        bandwidth_mbps: 0,
        max_connections: 0,
        tunnel: Some(TunnelContext {
            tunnel_id: 7,
            role: TunnelRole::Entry as i32,
            next_hop_addr: "2.2.2.2".into(),
            next_hop_inter_port: 30001,
            self_inter_port: 0,
            transport: "tcp".into(),
            self_ordinal: 0,
        }),
    };
    assert_eq!(r.tunnel.as_ref().unwrap().tunnel_id, 7);
    assert_eq!(r.tunnel.as_ref().unwrap().role, TunnelRole::Entry as i32);
    assert_eq!(r.tunnel.as_ref().unwrap().self_ordinal, 0);
}

#[test]
fn command_oneof_has_tunnel_credentials() {
    let c = Command {
        body: Some(Body::TunnelCredentials(TunnelCredentials {
            tunnel_id: 7,
            ordinal: 1,
            server_cert_pem: "S".into(),
            server_key_pem: "SK".into(),
            client_cert_pem: "C".into(),
            client_key_pem: "CK".into(),
            ca_pem: "CA".into(),
        })),
    };
    assert!(matches!(c.body, Some(Body::TunnelCredentials(_))));
    if let Some(Body::TunnelCredentials(ref tc)) = c.body {
        assert_eq!(tc.ca_pem, "CA");
    } else {
        panic!("expected TunnelCredentials body");
    }

    let r = Command {
        body: Some(Body::RevokeTunnelCredentials(RevokeTunnelCredentials {
            tunnel_id: 7,
        })),
    };
    assert!(matches!(r.body, Some(Body::RevokeTunnelCredentials(_))));
}
