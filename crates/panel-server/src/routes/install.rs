use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::path::PathBuf;

use crate::{error::ApiError, state::AppState};

#[derive(Deserialize)]
pub struct InstallScriptQuery {
    pub node: Option<i64>,
}

/// 返回参数化 bash 安装脚本。
/// 无需鉴权;token 通过 `--token=` 参数在使用者复制安装命令时一次性带入。
/// rate limit 由 routes/mod.rs 挂载时的 governor 中间件提供(Task 9 加)。
pub async fn install_sh(
    State(state): State<AppState>,
    Query(q): Query<InstallScriptQuery>,
) -> Result<Response, ApiError> {
    let node_id = q
        .node
        .ok_or_else(|| ApiError::BadRequest("缺少 ?node=<id> 参数".into()))?;

    // 从 system_settings 拉 agent_control_endpoint。
    let endpoint: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM system_settings WHERE key = 'agent_control_endpoint'",
    )
    .fetch_optional(&state.pool)
    .await?;
    let endpoint = endpoint.map(|(v,)| v).unwrap_or_default();

    // base URL 用于二进制下载;留空时脚本里报错。
    let base = state
        .config
        .panel_public_base_url
        .clone()
        .unwrap_or_else(|| "PANEL_PUBLIC_BASE_URL_NOT_SET".into());

    // 安全(H1):endpoint / base 以 base64 下发,脚本内 base64 -d 解码为 shell 变量。
    // base64 字母表仅 [A-Za-z0-9+/=],无 shell 元字符,从构造上消除命令注入。
    use base64::Engine as _;
    let endpoint_b64 = base64::engine::general_purpose::STANDARD.encode(endpoint.as_bytes());
    let base_b64 = base64::engine::general_purpose::STANDARD.encode(base.as_bytes());

    // 安全(H2):内联两架构二进制 sha256,脚本下载后校验;缺失则空串,脚本侧跳过并告警。
    let dist_dir = format!("{}/agent-dist", state.config.panel_data_dir);
    let sha_amd64 = sha256_of(&format!("{dist_dir}/node-agent-linux-amd64"))
        .await
        .unwrap_or_default();
    let sha_arm64 = sha256_of(&format!("{dist_dir}/node-agent-linux-arm64"))
        .await
        .unwrap_or_default();

    let script = render_install_sh(node_id, &endpoint_b64, &base_b64, &sha_amd64, &sha_arm64);

    let body = axum::body::Body::from(script);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("response build: {e}")))
}

/// 计算文件 sha256(hex)。文件缺失/读失败返回 None。
async fn sha256_of(path: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = tokio::fs::read(path).await.ok()?;
    Some(hex::encode(Sha256::digest(&bytes)))
}

fn render_install_sh(
    node_id: i64,
    control_endpoint_b64: &str,
    base_url_b64: &str,
    sha_amd64: &str,
    sha_arm64: &str,
) -> String {
    format!(
        r##"#!/usr/bin/env bash
# EMORELAY node-agent 一键安装脚本
# 生成于:本脚本由 panel-server `/install.sh` 端点动态渲染。
set -euo pipefail

TOKEN=""
CA_B64="" CERT_B64="" KEY_B64=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --token=*) TOKEN="${{1#*=}}"; shift ;;
    --token)   TOKEN="$2"; shift 2 ;;
    --ca-pem-b64=*)          CA_B64="${{1#*=}}"; shift ;;
    --client-cert-pem-b64=*) CERT_B64="${{1#*=}}"; shift ;;
    --client-key-pem-b64=*)  KEY_B64="${{1#*=}}"; shift ;;
    *) echo "unknown arg: $1" >&2; exit 64 ;;
  esac
done
if [[ -z "${{TOKEN:-}}" ]]; then
  echo "missing --token=<agent_token>" >&2
  exit 64
fi

# 解码面板下发的端点(base64,杜绝 shell 注入:base64 字母表无 shell 元字符)。
CONTROL_ENDPOINT="$(printf '%s' '{control_endpoint_b64}' | base64 -d)"
BASE_URL="$(printf '%s' '{base_url_b64}' | base64 -d)"
if [[ "$BASE_URL" == "PANEL_PUBLIC_BASE_URL_NOT_SET" ]]; then
  echo "panel-server is missing PANEL_PUBLIC_BASE_URL env; cannot bootstrap agent." >&2
  exit 78
fi

ARCH=""
EXPECTED_SHA=""
case "$(uname -m)" in
  x86_64|amd64)  ARCH=amd64; EXPECTED_SHA="{sha_amd64}" ;;
  aarch64|arm64) ARCH=arm64; EXPECTED_SHA="{sha_arm64}" ;;
  *) echo "unsupported arch: $(uname -m)" >&2; exit 70 ;;
esac

# 1. 下载二进制
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "downloading agent binary (linux-$ARCH)..."
curl -fsSL "${{BASE_URL}}/dist/node-agent-linux-${{ARCH}}" -o "$TMP/node-agent"
if [[ -n "$EXPECTED_SHA" ]]; then
  echo "${{EXPECTED_SHA}}  ${{TMP}}/node-agent" | sha256sum -c - \
    || {{ echo "agent binary sha256 mismatch — aborting" >&2; exit 1; }}
else
  echo "warning: panel has no expected sha256 for $ARCH; skipping integrity check" >&2
fi
install -m 0755 "$TMP/node-agent" /usr/local/bin/emorelay-agent

# 2. 写 env 文件
install -d -m 0755 /etc/emorelay

# 2.5 写 mTLS 凭据(若提供)。三者必须同时给。
if [[ -n "$CA_B64" && -n "$CERT_B64" && -n "$KEY_B64" ]]; then
  install -d -m 0700 /etc/emorelay/tls
  echo "$CA_B64"   | base64 -d > /etc/emorelay/tls/ca.pem
  echo "$CERT_B64" | base64 -d > /etc/emorelay/tls/client.pem
  echo "$KEY_B64"  | base64 -d > /etc/emorelay/tls/client-key.pem
  chmod 0600 /etc/emorelay/tls/*.pem
  TLS_ENV=$'AGENT_GRPC_CA_CERT=/etc/emorelay/tls/ca.pem\nAGENT_GRPC_CLIENT_CERT=/etc/emorelay/tls/client.pem\nAGENT_GRPC_CLIENT_KEY=/etc/emorelay/tls/client-key.pem'
elif [[ -f /etc/emorelay/tls/ca.pem && -f /etc/emorelay/tls/client.pem && -f /etc/emorelay/tls/client-key.pem ]]; then
  # 本次未带 cert,但节点已装过 mTLS 凭据 → 保留,避免重跑脚本把已在线节点降级。
  TLS_ENV=$'AGENT_GRPC_CA_CERT=/etc/emorelay/tls/ca.pem\nAGENT_GRPC_CLIENT_CERT=/etc/emorelay/tls/client.pem\nAGENT_GRPC_CLIENT_KEY=/etc/emorelay/tls/client-key.pem'
else
  TLS_ENV=""
fi

cat > /etc/emorelay/agent.env <<EOF
AGENT_NODE_ID={node_id}
AGENT_TOKEN=$TOKEN
AGENT_CONTROL_ENDPOINT=$CONTROL_ENDPOINT
AGENT_STATE_PATH=/var/lib/emorelay/agent-state.json
AGENT_DATA_DIR=/var/lib/emorelay
EOF
if [[ -n "$TLS_ENV" ]]; then printf '%s\n' "$TLS_ENV" >> /etc/emorelay/agent.env; fi
chmod 0600 /etc/emorelay/agent.env
install -d -m 0755 /var/lib/emorelay

# 3. 写 systemd unit
cat > /etc/systemd/system/emorelay-agent.service <<'EOF'
[Unit]
Description=EMORELAY node agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/emorelay/agent.env
ExecStart=/usr/local/bin/emorelay-agent
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
# /usr/local/bin 可写是 P10b 一键升级的前提(Agent 原子替换自身二进制)。
# 老版本装的节点缺这行,一键升级会 EROFS 失败,需手动补 + daemon-reload。
ReadWritePaths=/var/lib/emorelay /usr/local/bin

[Install]
WantedBy=multi-user.target
EOF

# 4. 启动
systemctl daemon-reload
systemctl enable --now emorelay-agent
sleep 1
systemctl status emorelay-agent --no-pager || true
echo
echo "done. agent connecting to $CONTROL_ENDPOINT for node #{node_id}"
"##,
        control_endpoint_b64 = control_endpoint_b64,
        base_url_b64 = base_url_b64,
        sha_amd64 = sha_amd64,
        sha_arm64 = sha_arm64,
        node_id = node_id,
    )
}

/// 提供预编译 agent 二进制下载。仅 amd64 / arm64 两个 target。
/// 严格白名单防 path traversal。
pub async fn dist_binary(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, ApiError> {
    let allowed = matches!(
        filename.as_str(),
        "node-agent-linux-amd64" | "node-agent-linux-arm64"
    );
    if !allowed {
        return Err(ApiError::NotFound);
    }
    let mut path = PathBuf::from(&state.config.panel_data_dir);
    path.push("agent-dist");
    path.push(&filename);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("response build: {e}")))
}
