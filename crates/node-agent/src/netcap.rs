use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

pub struct NetCapability {
    /// 0=unknown, 1=has, 2=no
    pub ipv4: i32,
    pub ipv6: i32,
}

/// UDP connect 路由探测（零流量零依赖）。
/// connect 仅查内核路由表,不发数据包。
pub fn probe() -> NetCapability {
    let ipv4 = probe_af(
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::from((Ipv4Addr::new(8, 8, 8, 8), 53)),
    );
    let ipv6 = probe_af(
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        SocketAddr::from(([0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888], 53)),
    );
    NetCapability { ipv4, ipv6 }
}

fn probe_af(bind: SocketAddr, target: SocketAddr) -> i32 {
    let Ok(sock) = UdpSocket::bind(bind) else {
        return 2;
    };
    match sock.connect(target) {
        Ok(()) => 1,
        Err(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_valid_values() {
        let cap = probe();
        assert!(cap.ipv4 == 1 || cap.ipv4 == 2);
        assert!(cap.ipv6 == 1 || cap.ipv6 == 2);
    }

    #[test]
    fn probe_v4_should_work_on_ci() {
        let cap = probe();
        assert_eq!(cap.ipv4, 1, "CI 环境应有 IPv4 路由");
    }
}
