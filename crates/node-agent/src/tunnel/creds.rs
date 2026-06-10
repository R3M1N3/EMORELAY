//! 隧道 TLS 凭据落盘(P3b)。布局:
//! ${AGENT_DATA_DIR}/tunnels/<tunnel_id>/hop-<ordinal>/{server.pem,server.key,client.pem,client.key,ca.pem}
//! 0600(Unix;Windows 跳过权限)。store 幂等覆盖(reconcile 重签重发);
//! RevokeTunnelCredentials 删整个 tunnels/<id>/。
use anyhow::{Context, Result};
use emorelay_common::control::v1::TunnelCredentials;
use std::path::{Path, PathBuf};

pub fn hop_dir(data_dir: &str, tunnel_id: i64, ordinal: u32) -> PathBuf {
    Path::new(data_dir)
        .join("tunnels")
        .join(tunnel_id.to_string())
        .join(format!("hop-{ordinal}"))
}

pub async fn store(data_dir: &str, c: &TunnelCredentials) -> Result<()> {
    let ordinal = u32::try_from(c.ordinal).context("negative tunnel hop ordinal")?;
    let dir = hop_dir(data_dir, c.tunnel_id, ordinal);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;
    // hop 目录先收紧到 0700:堵住 per-file 0600 chmod 落地前的 world-readable 窗口。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .await
            .with_context(|| format!("chmod 0700 {}", dir.display()))?;
    }
    for (name, content) in [
        ("server.pem", &c.server_cert_pem),
        ("server.key", &c.server_key_pem),
        ("client.pem", &c.client_cert_pem),
        ("client.key", &c.client_key_pem),
        ("ca.pem", &c.ca_pem),
    ] {
        let path = dir.join(name);
        tokio::fs::write(&path, content)
            .await
            .with_context(|| format!("write {name}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("chmod 0600 {name}"))?;
        }
    }
    Ok(())
}

pub async fn remove_tunnel(data_dir: &str, tunnel_id: i64) -> Result<()> {
    let dir = Path::new(data_dir).join("tunnels").join(tunnel_id.to_string());
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("remove tunnel creds dir"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emorelay_common::control::v1::TunnelCredentials;

    #[tokio::test]
    async fn store_writes_five_files_and_remove_cleans_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let data_dir = dir.path().display().to_string();
        let c = TunnelCredentials {
            tunnel_id: 7,
            ordinal: 1,
            server_cert_pem: "S".into(),
            server_key_pem: "SK".into(),
            client_cert_pem: "C".into(),
            client_key_pem: "CK".into(),
            ca_pem: "CA".into(),
        };
        store(&data_dir, &c).await.expect("store");
        let hop = hop_dir(&data_dir, 7, 1);
        for f in ["server.pem", "server.key", "client.pem", "client.key", "ca.pem"] {
            assert!(hop.join(f).exists(), "missing {f}");
        }
        remove_tunnel(&data_dir, 7).await.expect("remove");
        assert!(!hop.exists());
        // 幂等:再删不存在的目录不报错。
        remove_tunnel(&data_dir, 7).await.expect("remove twice");
    }
}
