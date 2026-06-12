//! 隧道凭据定期轮换。hop 证书短有效期(30 天,见 tls::issue::TUNNEL_CERT_VALIDITY_DAYS),
//! 本 sweeper 在凭据签发超过阈值(默认 20 天 ≈ 2/3 寿命)时自动重签下发并重启隧道规则,
//! 以「短有效期 + 自动轮换」替代隧道侧 CRL:泄漏窗口有上界,且无需 hop 间分发吊销表。
//! tcp 隧道无凭据,天然跳过;offline hop 由 reconcile 在重连时重放当前凭据。
use super::env_secs;
use crate::state::AppState;
use std::time::Duration;
use tracing::{info, warn};

pub fn spawn_tunnel_creds_sweeper(state: AppState) {
    let secs = env_secs("PANEL_TUNNEL_CREDS_ROTATE_SWEEP_SECS", 3600, 5);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(e) = rotate_tick_once(&state).await {
                warn!(error = ?e, "tunnel creds rotation sweep failed");
            }
        }
    });
}

/// 每 tick 扫描需轮换的活跃 tls/wss 隧道并逐条轮换。返回轮换条数。
/// pub 供集成测试直接调用(确定性,不等 interval)。
pub async fn rotate_tick_once(state: &AppState) -> anyhow::Result<u64> {
    let rotate_after = env_secs("PANEL_TUNNEL_CREDS_ROTATE_AFTER_SECS", 20 * 86400, 60);
    let cutoff_modifier = format!("-{rotate_after} seconds");

    // creds_rotated_at 为 NULL(0012 之前创建/从未下发)按 created_at 回落,
    // 保证老隧道也会被纳入轮换而不是永远豁免。
    let due: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tunnels \
         WHERE deleted_at IS NULL AND transport IN ('tls', 'wss') \
           AND COALESCE(creds_rotated_at, created_at) < datetime('now', ?) \
         ORDER BY id",
    )
    .bind(&cutoff_modifier)
    .fetch_all(&state.pool)
    .await?;

    let mut rotated = 0u64;
    for tid in due {
        let Some(tunnel) = crate::models::tunnel::Tunnel::find_by_id(&state.pool, tid).await? else {
            continue;
        };
        match crate::grpc::tunnel_dispatch::rotate_credentials_and_restart(state, &tunnel).await {
            Ok(dispatched) => {
                rotated += 1;
                info!(tunnel_id = tid, dispatched, "tunnel credentials rotated");
                crate::audit::record(
                    &state.pool,
                    None, // 系统动作,无 actor
                    "tunnel.creds_rotated",
                    Some("tunnel"),
                    Some(tid),
                    Some(&format!("auto rotation after {rotate_after}s; dispatched={dispatched}")),
                    true,
                    None,
                )
                .await;
            }
            Err(e) => {
                // 单条失败不拖垮整轮;creds_rotated_at 未刷新,下个 tick 自动重试。
                warn!(tunnel_id = tid, error = ?e, "tunnel credentials rotation failed");
            }
        }
    }
    Ok(rotated)
}
