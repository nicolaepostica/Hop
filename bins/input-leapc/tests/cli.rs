//! CLI smoke tests for `input-leapc`.
//!
//! Mirror of `input-leaps/tests/cli.rs`: both bins expose the same
//! fingerprint management UX so they are worth keeping symmetric.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("input-leapc").expect("binary in CARGO_BIN_EXE_*")
}

#[test]
fn version_flag() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("input-leapc "));
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
}

#[test]
fn fingerprint_add_list_remove_round_trip() {
    let dir = TempDir::new().unwrap();
    let cert_dir = dir.path().join("tls");
    let fp_db = dir.path().join("fp.toml");
    let dummy = "sha256:abcdef0011223344556677889900112233445566778899001122334455667788";

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["add", "server", dummy])
        .assert()
        .success();

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("server").and(predicate::str::contains(dummy)));

    bin()
        .args(["fingerprint"])
        .args(["--cert-dir", cert_dir.to_str().unwrap()])
        .args(["--fingerprint-db", fp_db.to_str().unwrap()])
        .args(["remove", "server"])
        .assert()
        .success();
}
