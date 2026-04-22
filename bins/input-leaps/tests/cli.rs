//! CLI smoke tests for `input-leaps`.
//!
//! These drive the compiled binary via `assert_cmd`, exercising the
//! `fingerprint` subcommand in a temp directory so nothing touches the
//! user's real config.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("input-leaps").expect("binary in CARGO_BIN_EXE_*")
}

#[test]
fn version_flag() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("input-leaps ").and(predicate::str::contains(".")));
}

#[test]
fn help_flag() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"));
}

#[test]
fn fingerprint_show_generates_pair_on_first_call() {
    let dir = TempDir::new().unwrap();
    let cert_dir = dir.path().join("tls");
    let fp_db = dir.path().join("fp.toml");

    let output = bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .arg("show")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown = String::from_utf8(output).unwrap();
    assert!(shown.trim().starts_with("sha256:"), "stdout was {shown}");
    assert!(cert_dir.join("cert.pem").exists());
    assert!(cert_dir.join("key.pem").exists());
}

#[test]
fn fingerprint_add_list_remove_round_trip() {
    let dir = TempDir::new().unwrap();
    let cert_dir = dir.path().join("tls");
    let fp_db = dir.path().join("fp.toml");
    // Use a fully-formed dummy fingerprint. It does not need to match
    // any real cert — the binary just stores it in the TOML file.
    let dummy = "sha256:0011223344556677889900112233445566778899001122334455667788990011";

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["add", "laptop", dummy])
        .assert()
        .success()
        .stdout(predicate::str::contains("added laptop"));

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("laptop").and(predicate::str::contains(dummy)));

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["remove", "laptop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed laptop"));

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("empty"));
}

#[test]
fn fingerprint_add_rejects_bad_fingerprint_format() {
    let dir = TempDir::new().unwrap();
    let cert_dir = dir.path().join("tls");
    let fp_db = dir.path().join("fp.toml");

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["add", "laptop", "not-a-fingerprint"])
        .assert()
        .failure();
}
