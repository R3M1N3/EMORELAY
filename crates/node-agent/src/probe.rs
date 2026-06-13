//! 逐段诊断探测(P1)。对面板下发的 target_host:target_port 做 count 次 TCP connect
//! 计时,聚合可达性/平均延迟/丢失率。白名单指令:只连面板已配置的链路节点/目标,
//! 不扫描第三方、不执行 shell。
use emorelay_common::control::v1::ProbeResult;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

/// 单次 connect 超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// count 上限,防滥用(面板侧也会限,这里再兜底)。
const MAX_COUNT: u32 = 20;

pub async fn run_probe(
    probe_id: String,
    target_host: &str,
    target_port: u16,
    count: u32,
) -> ProbeResult {
    let count = count.clamp(1, MAX_COUNT);
    let mut successes = 0u32;
    let mut total_latency_ms = 0f64;
    let mut last_err = String::new();

    for _ in 0..count {
        let start = Instant::now();
        match tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect((target_host, target_port)),
        )
        .await
        {
            Ok(Ok(_stream)) => {
                successes += 1;
                total_latency_ms += start.elapsed().as_secs_f64() * 1000.0;
            }
            Ok(Err(e)) => last_err = e.to_string(),
            Err(_) => last_err = "connect timeout".to_string(),
        }
    }

    let reachable = successes > 0;
    let avg_latency_ms = if successes > 0 {
        total_latency_ms / successes as f64
    } else {
        0.0
    };
    let loss_pct = (count - successes) as f64 / count as f64 * 100.0;
    ProbeResult {
        probe_id,
        reachable,
        avg_latency_ms,
        loss_pct,
        error: if reachable { String::new() } else { last_err },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn probe_reaches_listening_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // accept loop:让 connect 成功。
        tokio::spawn(async move {
            loop {
                if listener.accept().await.is_err() {
                    break;
                }
            }
        });
        let r = run_probe("p1".into(), "127.0.0.1", port, 3).await;
        assert_eq!(r.probe_id, "p1");
        assert!(r.reachable);
        assert_eq!(r.loss_pct, 0.0);
        assert!(r.error.is_empty());
    }

    #[tokio::test]
    async fn probe_unreachable_port_reports_loss() {
        // 绑一个端口再 drop 拿到大概率空闲的端口号。
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let r = run_probe("p2".into(), "127.0.0.1", port, 2).await;
        assert!(!r.reachable);
        assert_eq!(r.loss_pct, 100.0);
        assert!(!r.error.is_empty(), "全失败应带最后错误");
    }
}
