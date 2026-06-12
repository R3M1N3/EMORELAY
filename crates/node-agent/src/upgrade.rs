//! P10b Agent 一键升级。白名单固定动作:下载 → sha256 校验 → 原子替换自身 → exec 重启。
//! 不执行任何 shell;exec 仅替换为同路径的新二进制(systemd 单元/环境原样延续)。
//! 仅 unix 支持(替换运行中二进制 + execvp);其它平台直接报错。
use anyhow::{bail, Context as _, Result};
use emorelay_common::control::v1::UpgradeAgent;
use sha2::{Digest, Sha256};
use tracing::info;

/// 自身 arch → (下载文件名后缀, 对应 sha256 字段)。与 install.sh 的 uname -m 映射一致。
fn pick_artifact(cmd: &UpgradeAgent) -> Result<(&'static str, &str)> {
    match std::env::consts::ARCH {
        "x86_64" => Ok(("amd64", cmd.sha256_amd64.as_str())),
        "aarch64" => Ok(("arm64", cmd.sha256_arm64.as_str())),
        other => bail!("unsupported arch for upgrade: {other}"),
    }
}

/// 二进制大小上限:当前产物 ~10MB,留足余量;防 MITM/误配灌流打 OOM。
const MAX_BINARY_BYTES: usize = 256 * 1024 * 1024;
/// 下载总超时:升级走 spawn 不阻塞心跳,但也不允许无限挂住(占着 in-progress 锁)。
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// 执行升级。成功时 exec 不返回;返回 Ok(false) 表示幂等跳过。
pub async fn perform(cmd: &UpgradeAgent) -> Result<bool> {
    let current = env!("CARGO_PKG_VERSION");
    if cmd.version == current {
        info!(version = current, "upgrade skipped: already at target version");
        return Ok(false);
    }
    let (arch, want_sha) = pick_artifact(cmd)?;
    if want_sha.is_empty() {
        bail!("panel has no {arch} artifact (empty sha256)");
    }
    if cmd.base_url.trim().is_empty() {
        bail!("upgrade command carries empty base_url");
    }
    // 内容级幂等:面板 agent-dist 二进制可能与版本号标签不一致(手放产物),
    // 自身 sha 已等于目标 sha 时跳过,避免重复下载 + exec 断流。
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(self_bytes) = tokio::fs::read(&exe).await {
            if hex(&Sha256::digest(&self_bytes)).eq_ignore_ascii_case(want_sha) {
                info!("upgrade skipped: current binary already matches target sha256");
                return Ok(false);
            }
        }
    }
    let url = format!(
        "{}/dist/node-agent-linux-{arch}",
        cmd.base_url.trim_end_matches('/')
    );
    info!(%url, target = %cmd.version, "downloading agent upgrade");

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("build http client")?;
    let mut resp = client.get(&url).send().await.context("download request")?;
    if !resp.status().is_success() {
        bail!("download failed: HTTP {}", resp.status());
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BINARY_BYTES {
            bail!("artifact too large: {len} bytes");
        }
    }
    // 流式累计并强制上限(无 content-length 时唯一的兜底)。
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.context("download body")? {
        if bytes.len() + chunk.len() > MAX_BINARY_BYTES {
            bail!("artifact exceeds {MAX_BINARY_BYTES} bytes; aborting");
        }
        bytes.extend_from_slice(&chunk);
    }

    let got_sha = hex(&Sha256::digest(&bytes));
    if !got_sha.eq_ignore_ascii_case(want_sha) {
        bail!("sha256 mismatch: expected {want_sha}, got {got_sha}");
    }

    replace_and_exec(&bytes).await
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
async fn replace_and_exec(bytes: &[u8]) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use tracing::warn;

    let exe = std::env::current_exe().context("resolve current exe")?;
    // 同目录临时文件保证与目标同文件系统,rename 才是原子的。
    let staged = exe.with_extension("new");
    let backup = exe.with_extension("bak");

    tokio::fs::write(&staged, bytes).await.with_context(|| {
        format!(
            "write staged binary {} (EROFS? systemd unit needs ReadWritePaths={})",
            staged.display(),
            exe.parent().map(|p| p.display().to_string()).unwrap_or_default()
        )
    })?;
    tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .await
        .context("chmod staged binary")?;

    // current → .bak(旧版本保底,手动回滚用),staged → current。
    // 第二步失败时把 .bak 挪回来,保证路径上始终有可执行文件。
    tokio::fs::rename(&exe, &backup).await.context("backup current binary")?;
    if let Err(e) = tokio::fs::rename(&staged, &exe).await {
        warn!(error = ?e, "promote staged binary failed; rolling back");
        let _ = tokio::fs::rename(&backup, &exe).await;
        return Err(e).context("promote staged binary");
    }

    info!("upgrade staged; exec-restarting into new binary");
    // exec 替换进程映像:成功不返回。失败(极罕见,如新二进制损坏到不可加载)
    // 回滚 .bak 后再 exec 旧版本,保证服务不消失。
    let err = std::process::Command::new(&exe).args(std::env::args().skip(1)).exec();
    warn!(error = ?err, "exec new binary failed; rolling back and re-exec old");
    let _ = std::fs::rename(&backup, &exe);
    let err = std::process::Command::new(&exe).args(std::env::args().skip(1)).exec();
    bail!("exec rolled-back binary also failed: {err}");
}

#[cfg(not(unix))]
async fn replace_and_exec(_bytes: &[u8]) -> Result<bool> {
    bail!("agent upgrade is only supported on unix targets");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(version: &str, amd64: &str, arm64: &str) -> UpgradeAgent {
        UpgradeAgent {
            version: version.into(),
            base_url: "http://127.0.0.1:1".into(),
            sha256_amd64: amd64.into(),
            sha256_arm64: arm64.into(),
        }
    }

    #[tokio::test]
    async fn same_version_is_idempotent_skip() {
        let r = perform(&cmd(env!("CARGO_PKG_VERSION"), "x", "x")).await.unwrap();
        assert!(!r, "same version must skip without touching disk/network");
    }

    #[tokio::test]
    async fn empty_sha_for_own_arch_is_rejected() {
        // 两个 arch 的 sha 都给空,任何平台都该在下载前被拒。
        let e = perform(&cmd("999.0.0", "", "")).await.unwrap_err();
        let msg = format!("{e:#}");
        assert!(
            msg.contains("empty sha256") || msg.contains("unsupported arch"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn empty_base_url_is_rejected() {
        let mut c = cmd("999.0.0", "deadbeef", "deadbeef");
        c.base_url = "  ".into();
        let e = perform(&c).await.unwrap_err();
        assert!(format!("{e:#}").contains("base_url"), "{e:#}");
    }

    #[tokio::test]
    async fn unreachable_panel_fails_download_without_touching_self() {
        // 127.0.0.1:1 拒连:必须在替换自身前失败。
        let e = perform(&cmd("999.0.0", "deadbeef", "deadbeef")).await.unwrap_err();
        assert!(format!("{e:#}").contains("download"), "{e:#}");
    }

    #[test]
    fn hex_encodes_lowercase() {
        assert_eq!(hex(&[0xde, 0xad, 0x00]), "dead00");
    }
}
