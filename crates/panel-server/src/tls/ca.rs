//! 内部 CA 引导:首启自签一个 10 年 CA + 一张 server 叶子证书(供 gRPC mTLS 服务端用),
//! 全部落盘到 `tls_dir`。再次启动检测到 `ca.pem` 即复用磁盘上的四件套,**绝不重签**——
//! 否则已签发给 Agent 的 client 证书会因 CA 更换而集体失效。
//!
//! 证书均为 ECDSA P-256(`KeyPair::generate()` 在 rcgen 0.13 默认即此曲线)。
//! 私钥以 PKCS#8 PEM(`BEGIN PRIVATE KEY`)序列化。

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use time::{Duration, OffsetDateTime};

/// CA + server 叶子的 PEM 四件套,常驻内存供 gRPC TLS 配置与签发 Agent 证书(Task 2)复用。
pub struct CaBundle {
    /// CA 证书 PEM(自签根,信任锚)。
    pub ca_pem: String,
    /// CA 私钥 PEM(PKCS#8)。签发 Agent client 证书时需要。
    pub ca_key_pem: String,
    /// server 叶子证书 PEM(gRPC 服务端出示)。
    pub server_cert_pem: String,
    /// server 叶子私钥 PEM(PKCS#8)。
    pub server_key_pem: String,
}

/// 引导内部 CA:幂等。
///
/// - 若 `tls_dir/ca.pem` 已存在 → 从磁盘载入四件套并返回,**不重新生成**。
/// - 否则 → 自签 CA(10 年)与 server 叶子(5 年,SAN = 127.0.0.1 + localhost
///   + `public_host`),四件套写盘(Unix 下 0600),返回。
///
/// `public_host`:写入 server 证书 SAN 的对外主机名;`None` 时仅本地名(本地开发足够)。
pub fn bootstrap_ca(tls_dir: &str, public_host: Option<&str>) -> Result<Arc<CaBundle>> {
    let dir = Path::new(tls_dir);
    std::fs::create_dir_all(dir)
        .with_context(|| format!("创建 TLS 目录失败: {tls_dir}"))?;

    let ca_pem_path = dir.join("ca.pem");
    let ca_key_path = dir.join("ca.key");
    let server_pem_path = dir.join("server.pem");
    let server_key_path = dir.join("server.key");

    // 幂等:已有 CA 则直接复用磁盘四件套,绝不重签。
    if ca_pem_path.exists() {
        let bundle = CaBundle {
            ca_pem: read_pem(&ca_pem_path)?,
            ca_key_pem: read_pem(&ca_key_path)?,
            server_cert_pem: read_pem(&server_pem_path)?,
            server_key_pem: read_pem(&server_key_path)?,
        };
        return Ok(Arc::new(bundle));
    }

    let now = OffsetDateTime::now_utc();

    // ---- CA:自签,10 年,可签证书与 CRL ----
    let mut ca_params = CertificateParams::new(Vec::new())
        .context("构造 CA CertificateParams 失败")?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "EMORELAY Internal CA");
    ca_params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    ca_params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    ca_params.key_usages.push(KeyUsagePurpose::CrlSign);
    ca_params.not_before = now - Duration::days(1);
    ca_params.not_after = now + Duration::days(3650);

    let ca_key = KeyPair::generate().context("生成 CA 密钥对失败")?;
    let ca_key_pem = ca_key.serialize_pem();
    // self_signed 按值消费 ca_params;ca_cert 内部保留 params,后续 signed_by 直接用 ca_cert + ca_key。
    let ca_cert = ca_params.self_signed(&ca_key).context("CA 自签失败")?;
    let ca_pem = ca_cert.pem();

    // ---- server 叶子:ServerAuth,5 年,由 CA 签 ----
    let mut sans = vec!["127.0.0.1".to_string(), "localhost".to_string()];
    if let Some(host) = public_host {
        sans.push(host.to_string());
    }
    let mut srv_params =
        CertificateParams::new(sans).context("构造 server CertificateParams 失败")?;
    srv_params
        .distinguished_name
        .push(DnType::CommonName, "EMORELAY panel-server");
    srv_params.use_authority_key_identifier_extension = true;
    srv_params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    srv_params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ServerAuth);
    srv_params.not_before = now - Duration::days(1);
    srv_params.not_after = now + Duration::days(1825);

    let srv_key = KeyPair::generate().context("生成 server 密钥对失败")?;
    let srv_cert = srv_params
        .signed_by(&srv_key, &ca_cert, &ca_key)
        .context("CA 签发 server 叶子失败")?;
    let server_cert_pem = srv_cert.pem();
    let server_key_pem = srv_key.serialize_pem();

    // 四件套写盘(私钥 Unix 0600)。
    write_pem(&ca_pem_path, &ca_pem)?;
    write_pem(&ca_key_path, &ca_key_pem)?;
    write_pem(&server_pem_path, &server_cert_pem)?;
    write_pem(&server_key_path, &server_key_pem)?;

    Ok(Arc::new(CaBundle {
        ca_pem,
        ca_key_pem,
        server_cert_pem,
        server_key_pem,
    }))
}

fn read_pem(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("读取 PEM 失败: {}", path.display()))
}

/// 写 PEM 文件。Unix 下用 `OpenOptions::mode(0o600)` 让文件**自创建即 0600**——
/// 而非先 write(按 umask 常落 0644)再 chmod,避免私钥短暂以全局可读暴露的竞态窗口。
/// 四个文件统一走此路径:公开证书虽不敏感,但统一处理更简单。Windows 无 POSIX 权限,直接写。
fn write_pem(path: &Path, contents: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("以 0600 创建 PEM 失败: {}", path.display()))?;
        f.write_all(contents.as_bytes())
            .with_context(|| format!("写入 PEM 失败: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
            .with_context(|| format!("写入 PEM 失败: {}", path.display()))?;
    }
    Ok(())
}
