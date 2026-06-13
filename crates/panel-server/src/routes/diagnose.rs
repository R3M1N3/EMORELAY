//! 逐段链路诊断(P1,对标 flux diagnose)。把一条规则/隧道的链路拆成「源节点 → 目标」
//! 的若干段,对每段下发 Probe 到源节点,收集 TCP 可达性/延迟/丢失,定位哪一段断了。
//! 请求-响应:REST 注册 probe 等待者 → dispatch Probe → Agent 回报 → resolve。
use crate::{
    auth::extractor::AuthUser,
    error::{ApiError, ApiResult},
    models::{rule::Rule, tunnel::Tunnel},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use emorelay_common::control::v1::{command::Body, Command, Probe};
use serde::Serialize;
use std::time::Duration;

/// 每段探测次数与等待上限。
const PROBE_COUNT: u32 = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 一段链路:从 source 节点 connect 到 target_host:target_port。
/// pre_error 非空时不下发探测,直接产出该错误段(如配置不全的 hop),避免漏段掩盖故障。
struct Segment {
    source_node_id: i64,
    source_node_name: String,
    label: String,
    target_host: String,
    target_port: i64,
    pre_error: Option<String>,
}

#[derive(Serialize)]
pub struct SegmentResult {
    pub label: String,
    pub source_node_id: i64,
    pub source_node_name: String,
    pub target: String,
    /// 命令是否送达源节点(节点在线)。false 时其余字段无意义。
    pub dispatched: bool,
    pub reachable: bool,
    pub avg_latency_ms: f64,
    pub loss_pct: f64,
    pub error: String,
}

#[derive(Serialize)]
pub struct DiagnoseResponse {
    pub segments: Vec<SegmentResult>,
}

/// 隧道 hop 链(按 ordinal)→ 相邻段:hop_i 节点 connect hop_{i+1} 的 public_ip:inter_port。
async fn tunnel_chain_segments(state: &AppState, tunnel_id: i64) -> ApiResult<Vec<Segment>> {
    #[derive(sqlx::FromRow)]
    struct HopRow {
        ordinal: i64,
        node_id: i64,
        name: String,
        public_ip: String,
        inter_port: Option<i64>,
    }
    let hops: Vec<HopRow> = sqlx::query_as(
        "SELECT th.ordinal, th.node_id, n.name, n.public_ip, th.inter_port \
         FROM tunnel_hops th JOIN nodes n ON n.id = th.node_id \
         WHERE th.tunnel_id = ? ORDER BY th.ordinal",
    )
    .bind(tunnel_id)
    .fetch_all(&state.pool)
    .await?;

    let mut segs = Vec::new();
    for pair in hops.windows(2) {
        let (src, dst) = (&pair[0], &pair[1]);
        let label = format!("第 {} 跳 → 第 {} 跳", src.ordinal + 1, dst.ordinal + 1);
        // 下一跳缺 inter_port 是一种链路故障,产出错误段而非跳过(否则用户只见少一段)。
        let (target_port, pre_error) = match dst.inter_port {
            Some(p) => (p, None),
            None => (0, Some("该跳未分配中继端口（配置不全）".to_string())),
        };
        segs.push(Segment {
            source_node_id: src.node_id,
            source_node_name: src.name.clone(),
            label,
            target_host: dst.public_ip.clone(),
            target_port,
            pre_error,
        });
    }
    Ok(segs)
}

/// 末段(出口节点 → 业务目标);隧道纯链路诊断不含此段。
fn exit_to_target_segment(exit_node_id: i64, exit_name: String, host: &str, port: i64) -> Segment {
    Segment {
        source_node_id: exit_node_id,
        source_node_name: exit_name,
        label: "出口 → 目标".to_string(),
        target_host: host.to_string(),
        target_port: port,
        pre_error: None,
    }
}

/// 并发对每段下发 Probe 并收集结果(保序)。
async fn run_segments(state: &AppState, segments: Vec<Segment>) -> Vec<SegmentResult> {
    let mut set = tokio::task::JoinSet::new();
    for (idx, seg) in segments.into_iter().enumerate() {
        let state = state.clone();
        set.spawn(async move { (idx, run_one(&state, seg).await) });
    }
    let mut out: Vec<Option<SegmentResult>> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok((idx, r)) = res {
            if idx >= out.len() {
                out.resize_with(idx + 1, || None);
            }
            out[idx] = Some(r);
        }
    }
    out.into_iter().flatten().collect()
}

async fn run_one(state: &AppState, seg: Segment) -> SegmentResult {
    let target = fmt_target(&seg.target_host, seg.target_port);
    // 预置错误段(如 hop 配置不全):不下发探测,直接产出错误结果。
    if let Some(err) = seg.pre_error {
        return SegmentResult {
            label: seg.label,
            source_node_id: seg.source_node_id,
            source_node_name: seg.source_node_name,
            target,
            dispatched: false,
            reachable: false,
            avg_latency_ms: 0.0,
            loss_pct: 100.0,
            error: err,
        };
    }
    let (probe_id, rx) = state.register_probe();
    let cmd = Command {
        body: Some(Body::Probe(Probe {
            probe_id: probe_id.clone(),
            target_host: seg.target_host.clone(),
            target_port: seg.target_port as u32,
            count: PROBE_COUNT,
        })),
    };
    if !state.dispatcher.dispatch(seg.source_node_id, cmd) {
        state.cancel_probe(&probe_id);
        return SegmentResult {
            label: seg.label,
            source_node_id: seg.source_node_id,
            source_node_name: seg.source_node_name,
            target,
            dispatched: false,
            reachable: false,
            avg_latency_ms: 0.0,
            loss_pct: 100.0,
            error: "源节点离线，无法探测".to_string(),
        };
    }
    match tokio::time::timeout(PROBE_TIMEOUT, rx).await {
        // Agent 上报数值不可全信:clamp 防离谱展示(对齐 stats 的防御基线)。
        Ok(Ok(r)) => SegmentResult {
            label: seg.label,
            source_node_id: seg.source_node_id,
            source_node_name: seg.source_node_name,
            target,
            dispatched: true,
            reachable: r.reachable,
            avg_latency_ms: r.avg_latency_ms.max(0.0),
            loss_pct: r.loss_pct.clamp(0.0, 100.0),
            error: r.error,
        },
        _ => {
            state.cancel_probe(&probe_id);
            SegmentResult {
                label: seg.label,
                source_node_id: seg.source_node_id,
                source_node_name: seg.source_node_name,
                target,
                dispatched: true,
                reachable: false,
                avg_latency_ms: 0.0,
                loss_pct: 100.0,
                error: "探测超时（Agent 未在限时内回报）".to_string(),
            }
        }
    }
}

pub fn fmt_target(host: &str, port: i64) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// POST /api/rules/{id}/diagnose
pub async fn diagnose_rule(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<DiagnoseResponse>> {
    let rule = Rule::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // owner 校验:普通用户只能诊断自己的规则。
    if !auth.is_admin() && rule.user_id != auth.0.sub {
        return Err(ApiError::NotFound);
    }

    let segments = match rule.tunnel_id {
        None => vec![Segment {
            source_node_id: rule.node_id,
            source_node_name: node_name(&state, rule.node_id).await,
            label: "节点 → 目标".to_string(),
            target_host: rule.target_host.clone(),
            target_port: rule.target_port,
            pre_error: None,
        }],
        Some(tid) => {
            let mut segs = tunnel_chain_segments(&state, tid).await?;
            // 末段:出口节点 → 业务目标。出口 = 最大 ordinal 的 hop。
            if let Some((exit_id, exit_name)) = exit_hop(&state, tid).await? {
                segs.push(exit_to_target_segment(
                    exit_id,
                    exit_name,
                    &rule.target_host,
                    rule.target_port,
                ));
            }
            segs
        }
    };

    Ok(Json(DiagnoseResponse {
        segments: run_segments(&state, segments).await,
    }))
}

/// POST /api/tunnels/{id}/diagnose（admin only）。仅诊断 hop 链连通性(无业务目标)。
pub async fn diagnose_tunnel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<DiagnoseResponse>> {
    auth.require_admin()?;
    Tunnel::find_by_id(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let segments = tunnel_chain_segments(&state, id).await?;
    Ok(Json(DiagnoseResponse {
        segments: run_segments(&state, segments).await,
    }))
}

async fn node_name(state: &AppState, node_id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE id = ?")
        .bind(node_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| format!("节点 #{node_id}"))
}

async fn exit_hop(state: &AppState, tunnel_id: i64) -> ApiResult<Option<(i64, String)>> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT th.node_id, n.name FROM tunnel_hops th JOIN nodes n ON n.id = th.node_id \
         WHERE th.tunnel_id = ? ORDER BY th.ordinal DESC LIMIT 1",
    )
    .bind(tunnel_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(row)
}
