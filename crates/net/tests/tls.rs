//! Tests for the TLS identity loader.
//!
//! Covers: first-run generation, idempotent reload, Unix key
//! permissions, and malformed PEM rejection.

use std::fs;
use std::path::Path;

use input_leap_net::{load_or_generate_cert, TlsError};
use tempfile::TempDir;

#[test]
fn first_run_generates_cert_and_key() {
    let dir = TempDir::new().unwrap();
    let identity = load_or_generate_cert(dir.path()).expect("generate");
    assert!(identity.cert_path.exists());
    assert!(identity.key_path.exists());
    assert_eq!(identity.cert_path, dir.path().join("cert.pem"));
    assert_eq!(identity.key_path, dir.path().join("key.pem"));
    // Fingerprint uses the sha256:<hex> format.
    let shown = identity.fingerprint.to_string();
    assert!(shown.starts_with("sha256:"), "got {shown}");
    assert_eq!(shown.len(), "sha256:".len() + 64);
}

#[test]
fn second_call_reuses_existing_pair() {
    let dir = TempDir::new().unwrap();
    let first = load_or_generate_cert(dir.path()).expect("first");
    let first_cert_bytes = fs::read(&first.cert_path).unwrap();
    let first_key_bytes = fs::read(&first.key_path).unwrap();

    let second = load_or_generate_cert(dir.path()).expect("second");
    // Files are not rewritten — bytes are identical.
    assert_eq!(first_cert_bytes, fs::read(&second.cert_path).unwrap());
    assert_eq!(first_key_bytes, fs::read(&second.key_path).unwrap());
    // Fingerprint is therefore identical too.
    assert_eq!(first.fingerprint, second.fingerprint);
}

#[cfg(unix)]
#[test]
fn private_key_has_0600_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let identity = load_or_generate_cert(dir.path()).expect("generate");
    let mode = fs::metadata(&identity.key_path)
        .unwrap()
        .permissions()
        .mode();
    // Mask out file-type bits; only the user has any access.
    assert_eq!(
        mode & 0o777,
        0o600,
        "key.pem mode should be 0600, got {:o}",
        mode & 0o777
    );
}

#[test]
fn malformed_cert_pem_is_rejected() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "cert.pem", "not a cert");
    write(dir.path(), "key.pem", "not a key");

    match load_or_generate_cert(dir.path()) {
        Err(TlsError::MalformedPem { path }) => {
            assert_eq!(path.file_name().unwrap(), "cert.pem");
        }
        other => panic!("expected MalformedPem, got {other:?}"),
    }
}

#[test]
fn clone_produces_equivalent_identity() {
    let dir = TempDir::new().unwrap();
    let identity = load_or_generate_cert(dir.path()).expect("generate");
    let cloned = identity.clone();
    assert_eq!(cloned.fingerprint, identity.fingerprint);
    assert_eq!(cloned.chain.len(), identity.chain.len());
    assert_eq!(cloned.chain[0].as_ref(), identity.chain[0].as_ref());
}

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}
