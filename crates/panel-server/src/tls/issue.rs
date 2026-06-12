//! 为节点签发 mTLS client 证书(P3a)。SAN = node-<id>.emorelay.internal,
//! EKU = ClientAuth,由内置 CA 签名。返回明文 cert+key(一次性下发)+ serial + fingerprint(落 DB)。
use crate::tls::ca::{CaBundle, CA_COMMON_NAME};
use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SerialNumber,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

pub struct IssuedCert {
    pub cert_pem: String,
    pub key_pem: String,
    /// 证书 serial,十六进制串(落 nodes.cert_serial)。
    pub serial: String,
    /// 证书 DER 的 SHA-256,64 位十六进制(落 nodes.cert_fingerprint;CRL 比对用)。
    pub fingerprint: String,
}

/// 由内置 CA 签发一张节点 client 叶子证书。
///
/// CaBundle 只存 PEM 字符串(无内存态 rcgen 对象),故每次签发都从 PEM 重建 issuer。
/// 注:rcgen 0.13 的 `CertificateParams::from_ca_cert_pem` 被 `x509-parser` feature 门控,
/// 本项目未启用该 feature(避免新增依赖),因此改为**确定性重建** issuer 参数:
/// - `issuer_key`:从 `ca.key` PEM 重建的 CA 私钥,是叶子签名的实际产生者 → 链可被原始
///   `ca.pem` 验证;且其公钥与原 CA 一致 → SKI 一致 → 叶子的 AKI 与 CA 的 SKI 对齐。
/// - `issuer_cert`:用与 `bootstrap_ca` **完全相同**的 DN/扩展重建 CA 参数,再以该 CA 私钥
///   重新自签。`signed_by` 仅从中取 issuer 的 DN 与 key_identifier_method 写入叶子的
///   issuer 字段与 AKI(见 rcgen certificate.rs::signed_by),故只要 DN 与 CA 一致即可。
///
/// 低频操作(创建/轮换节点),毫秒级开销可接受。
pub fn issue_client_cert(ca: &CaBundle, node_id: i64) -> Result<IssuedCert> {
    let san = format!("node-{node_id}.emorelay.internal");

    // 从 PEM 重建 CA 私钥;并据其重建 issuer 证书(DN/扩展与 bootstrap 对齐)。
    let issuer_key =
        KeyPair::from_pem(&ca.ca_key_pem).context("从 PEM 重建 CA 私钥失败")?;
    let issuer_cert = rebuild_issuer_cert(&issuer_key).context("重建 issuer 证书失败")?;

    // 叶子私钥(ECDSA P-256)。
    let child_key = KeyPair::generate().context("生成节点密钥对失败")?;

    let now = OffsetDateTime::now_utc();
    // 随机非零 serial(| 1 保证最低位置 1,绝不为 0)。
    let serial_u64 = rand::random::<u64>() | 1;

    let mut params =
        CertificateParams::new(vec![san]).context("构造节点 CertificateParams 失败")?;
    params.is_ca = IsCa::NoCa;
    params
        .distinguished_name
        .push(DnType::CommonName, format!("node-{node_id}"));
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params
        .extended_key_usages
        .push(ExtendedKeyUsagePurpose::ClientAuth);
    params.serial_number = Some(SerialNumber::from(serial_u64));
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(1825); // 5 年

    // signed_by:叶子公钥 + issuer 证书 + issuer 私钥(CA 私钥)。
    // 签名由 CA 私钥产生 → 可被原始 ca.pem 验证。
    let child = params
        .signed_by(&child_key, &issuer_cert, &issuer_key)
        .context("CA 签发节点 client 叶子失败")?;

    let cert_pem = child.pem();
    let key_pem = child_key.serialize_pem();
    let fingerprint = hex::encode(Sha256::digest(child.der()));

    Ok(IssuedCert {
        cert_pem,
        key_pem,
        serial: format!("{serial_u64:016x}"),
        fingerprint,
    })
}

/// 隧道 hop 的 TLS 凭据(P3b 数据面)。server/client 各一张叶子,
/// SAN 同为 tunnel-<id>-hop-<ordinal>.emorelay.internal(dial 方 SNI 校验 server SAN;
/// client 叶子链验证即可,SAN 不参与 server 端校验)。不入 DB,即时签发即时下发。
pub struct TunnelHopCerts {
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
}

/// 隧道 hop 叶子证书有效期。短有效期 + sweeper 定期轮换替代隧道侧 CRL:
/// 即便凭据泄漏,窗口也只有 30 天;轮换(默认 20 天)由 panel 自动重签下发。
pub const TUNNEL_CERT_VALIDITY_DAYS: i64 = 30;

pub fn issue_tunnel_hop_certs(ca: &CaBundle, tunnel_id: i64, ordinal: i64) -> Result<TunnelHopCerts> {
    let san = format!("tunnel-{tunnel_id}-hop-{ordinal}.emorelay.internal");
    let issuer_key = KeyPair::from_pem(&ca.ca_key_pem).context("从 PEM 重建 CA 私钥失败")?;
    let issuer_cert = rebuild_issuer_cert(&issuer_key).context("重建 issuer 证书失败")?;

    let (server_cert_pem, server_key_pem) =
        issue_tunnel_leaf(&san, ExtendedKeyUsagePurpose::ServerAuth, &issuer_cert, &issuer_key)?;
    let (client_cert_pem, client_key_pem) =
        issue_tunnel_leaf(&san, ExtendedKeyUsagePurpose::ClientAuth, &issuer_cert, &issuer_key)?;

    Ok(TunnelHopCerts {
        server_cert_pem,
        server_key_pem,
        client_cert_pem,
        client_key_pem,
    })
}

fn issue_tunnel_leaf(
    san: &str,
    eku: ExtendedKeyUsagePurpose,
    issuer_cert: &Certificate,
    issuer_key: &KeyPair,
) -> Result<(String, String)> {
    let key = KeyPair::generate().context("生成隧道叶子密钥失败")?;
    let now = OffsetDateTime::now_utc();
    let mut params = CertificateParams::new(vec![san.to_string()])
        .context("构造隧道叶子 CertificateParams 失败")?;
    params.is_ca = IsCa::NoCa;
    params.distinguished_name.push(DnType::CommonName, san);
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    params.extended_key_usages.push(eku);
    params.serial_number = Some(SerialNumber::from(rand::random::<u64>() | 1));
    params.use_authority_key_identifier_extension = true;
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(TUNNEL_CERT_VALIDITY_DAYS);
    let cert = params
        .signed_by(&key, issuer_cert, issuer_key)
        .context("CA 签发隧道叶子失败")?;
    Ok((cert.pem(), key.serialize_pem()))
}

/// 重建 issuer 证书,仅供 `signed_by` 取用。
///
/// **签发不变式(改 `bootstrap_ca` 前必读)**:叶子要链到本 CA,靠两点对齐——
/// (1) issuer 的 DN(`CA_COMMON_NAME`)与 CA 一致;(2) 默认 `KeyIdMethod::Sha256`
/// (本函数与 `bootstrap_ca` 都不显式设 `key_identifier_method`)。叶子 AKI 由 CA 公钥经
/// 该 Sha256 方法推导,链到 CA 的 SKI。若 `bootstrap_ca` 改了 CA 的 DN 或设了非默认
/// `key_identifier_method`,这里必须同步,否则 AKI↔SKI 链**静默断裂**(签发不报错,mTLS 失败)。
///
/// 另:rcgen 0.13.2 的 `signed_by`(certificate.rs)只从 issuer 证书读 `distinguished_name`
/// 与 `key_identifier_method`,**不读 issuer 的 key_usages**,故这里无需设 key_usages。
fn rebuild_issuer_cert(issuer_key: &KeyPair) -> Result<Certificate> {
    let mut ca_params = CertificateParams::new(Vec::new())
        .context("重建 CA CertificateParams 失败")?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, CA_COMMON_NAME);
    ca_params
        .self_signed(issuer_key)
        .context("重建 issuer 证书自签失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::DnValue;

    /// 钉死「重建 issuer 的 DN CommonName == CA subject DN」这条签发不变式:
    /// 若日后改了 `bootstrap_ca` 的 CA DN 而忘了同步 `rebuild_issuer_cert`,CI 立刻报错
    /// (无需 openssl)。注:`DistinguishedName::push(CommonName, &str)` 存为 `Utf8String`。
    #[test]
    fn rebuilt_issuer_dn_matches_ca_common_name() {
        let ca_key = KeyPair::generate().expect("ca key");
        let issuer = rebuild_issuer_cert(&ca_key).expect("rebuild issuer");
        let cn = issuer
            .params()
            .distinguished_name
            .get(&DnType::CommonName)
            .expect("issuer 缺 CommonName");
        assert_eq!(cn, &DnValue::Utf8String(CA_COMMON_NAME.to_string()));
    }
}
