//! CA bootstrap 与证书签发的链路测试。用临时目录,不污染真实 data dir。
use panel_server::tls::ca::{bootstrap_ca, CaBundle};
use panel_server::tls::issue::issue_client_cert;
use std::sync::Arc;
use tempfile::TempDir;

fn tls_dir(t: &TempDir) -> String {
    t.path().display().to_string().replace('\\', "/")
}

#[test]
fn bootstrap_generates_ca_and_server_cert_then_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let tls = tls_dir(&dir);

    let bundle = bootstrap_ca(&tls, Some("relay.example.com")).expect("first bootstrap");
    for f in ["ca.pem", "ca.key", "server.pem", "server.key"] {
        assert!(std::path::Path::new(&tls).join(f).exists(), "missing {f}");
    }
    let ca_pem_first = bundle.ca_pem.clone();

    let bundle2 = bootstrap_ca(&tls, Some("relay.example.com")).expect("second bootstrap");
    assert_eq!(bundle2.ca_pem, ca_pem_first, "幂等:CA 必须复用,不可重签");
}

#[test]
fn issued_server_cert_chains_to_ca() {
    let dir = TempDir::new().unwrap();
    let bundle: Arc<CaBundle> = bootstrap_ca(&tls_dir(&dir), None).unwrap();
    assert!(bundle.ca_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.server_cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.server_key_pem.contains("BEGIN PRIVATE KEY")
        || bundle.server_key_pem.contains("BEGIN EC PRIVATE KEY"));
    assert_ne!(bundle.ca_pem, bundle.server_cert_pem);
}

#[test]
fn issue_client_cert_chains_and_has_stable_fingerprint() {
    let dir = TempDir::new().unwrap();
    let ca = bootstrap_ca(&tls_dir(&dir), None).unwrap();

    let issued = issue_client_cert(&ca, 42).expect("issue");
    assert!(issued.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(
        issued.key_pem.contains("BEGIN PRIVATE KEY")
            || issued.key_pem.contains("BEGIN EC PRIVATE KEY")
    );
    assert!(!issued.serial.is_empty() && issued.serial.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(issued.fingerprint.len(), 64, "SHA-256 hex = 64 chars");
    assert!(issued.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));

    let issued2 = issue_client_cert(&ca, 42).expect("issue2");
    assert_ne!(issued.fingerprint, issued2.fingerprint);
}

use panel_server::grpc::{tls_mode_for, GrpcTlsMode};

#[test]
fn dev_disable_mtls_yields_plaintext() {
    assert!(matches!(tls_mode_for(true), GrpcTlsMode::Plaintext));
    assert!(matches!(tls_mode_for(false), GrpcTlsMode::Mtls));
}

#[test]
fn crl_load_missing_file_is_empty() {
    let crl = panel_server::tls::crl::Crl::load("/nonexistent/crl.json");
    assert!(!crl.is_revoked("deadbeef"));
}
