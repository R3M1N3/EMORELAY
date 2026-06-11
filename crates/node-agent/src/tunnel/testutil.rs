//! 测试专用:rcgen 自签 CA + 为相邻两 hop 写凭据目录(模拟 TunnelCredentials 落盘)。
#![cfg(test)]
use emorelay_common::control::v1::TunnelCredentials;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};

fn make_ca() -> (KeyPair, Certificate) {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(Vec::new()).unwrap();
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.distinguished_name.push(DnType::CommonName, "test-tunnel-ca");
    let cert = p.self_signed(&key).unwrap();
    (key, cert)
}

fn issue_leaf(
    san: &str,
    eku: ExtendedKeyUsagePurpose,
    ca_cert: &Certificate,
    ca_key: &KeyPair,
) -> (String, String) {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(vec![san.to_string()]).unwrap();
    p.is_ca = IsCa::NoCa;
    p.key_usages.push(KeyUsagePurpose::DigitalSignature);
    p.extended_key_usages.push(eku);
    let cert = p.signed_by(&key, ca_cert, ca_key).unwrap();
    (cert.pem(), key.serialize_pem())
}

/// 为同一 CA 下的多个 hop 目录写凭据。spec = (dir_ordinal, client_san_ordinal):
/// server SAN 始终 = tunnel-<id>-hop-<dir_ordinal>;client SAN = tunnel-<id>-hop-<client_san_ordinal>
/// (两者不一致用于构造「链合法但 SAN 不属于上一跳」的负路径)。
pub async fn write_hop_creds_matrix(data_dir: &str, tunnel_id: i64, specs: &[(u32, u32)]) {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let (ca_key, ca_cert) = make_ca();
    let ca_pem = ca_cert.pem();
    for (dir_ordinal, client_san_ordinal) in specs {
        let server_san = format!("tunnel-{tunnel_id}-hop-{dir_ordinal}.emorelay.internal");
        let client_san = format!("tunnel-{tunnel_id}-hop-{client_san_ordinal}.emorelay.internal");
        let (server_cert_pem, server_key_pem) =
            issue_leaf(&server_san, ExtendedKeyUsagePurpose::ServerAuth, &ca_cert, &ca_key);
        let (client_cert_pem, client_key_pem) =
            issue_leaf(&client_san, ExtendedKeyUsagePurpose::ClientAuth, &ca_cert, &ca_key);
        crate::tunnel::creds::store(
            data_dir,
            &TunnelCredentials {
                tunnel_id,
                ordinal: *dir_ordinal as i32,
                server_cert_pem,
                server_key_pem,
                client_cert_pem,
                client_key_pem,
                ca_pem: ca_pem.clone(),
            },
        )
        .await
        .expect("write test hop creds");
    }
}

/// 为相邻两 hop 各写一套凭据(同一 CA,client SAN = 自身 hop,正路径)。
pub async fn write_hop_creds_pair(data_dir: &str, tunnel_id: i64, ordinal_a: u32, ordinal_b: u32) {
    write_hop_creds_matrix(data_dir, tunnel_id, &[(ordinal_a, ordinal_a), (ordinal_b, ordinal_b)])
        .await
}
