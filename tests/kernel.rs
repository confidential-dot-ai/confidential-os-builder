//! Integration tests for steep kernel + kernel_cache.
//!
//! These run a real kernel build under systemd-nspawn. Mark with `#[ignore]`
//! so `cargo test` doesn't trigger a 10+ minute build.
//!
//! Run with: `cargo test --test kernel -- --ignored`
//!
//! Requires:
//!   - `kernel/version`, `kernel/required.config`, `kernel/hardening.config`,
//!     `kernel/config-x86_64.snapshot` checked in
//!   - `mkosi/kernel-builder/` config in place
//!   - sudo + systemd-nspawn available
//!   - network access to cdn.kernel.org
//!
//! Each test runs in its own temp output dir (no interference with `output/kernel/`).

use assert_cmd::Command;
use std::path::PathBuf;

fn binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("steep")
}

#[test]
#[ignore]
fn kernel_build_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();
    assert!(out.join("vmlinuz").exists());
    assert!(out.join("manifest.json").exists());
}

#[test]
#[ignore]
fn kernel_build_is_reproducible() {
    let tmp1 = tempfile::TempDir::new().unwrap();
    let tmp2 = tempfile::TempDir::new().unwrap();

    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(tmp1.path().join("kernel"))
        .assert()
        .success();
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(tmp2.path().join("kernel"))
        .assert()
        .success();

    let h1 = sha256(&tmp1.path().join("kernel/vmlinuz"));
    let h2 = sha256(&tmp2.path().join("kernel/vmlinuz"));
    assert_eq!(h1, h2, "vmlinuz not reproducible across builds");
}

#[test]
#[ignore]
fn kernel_cache_hits_on_second_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();

    let m1 = std::fs::metadata(out.join("vmlinuz")).unwrap();
    let mtime1 = m1.modified().unwrap();
    // Sleep a second so any rebuild has a strictly later mtime.
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Second run with the same output dir should hit cache.
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();
    let m2 = std::fs::metadata(out.join("vmlinuz")).unwrap();
    let mtime2 = m2.modified().unwrap();
    assert_eq!(
        mtime1, mtime2,
        "vmlinuz was rewritten — cache miss when hit expected"
    );
}

#[test]
#[ignore]
fn kernel_drift_fails_without_update_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    // First, do a clean build to populate the cache + snapshot.
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();

    // Modify the snapshot so the next build's resolved config diverges.
    let snap = std::fs::read_to_string("kernel/config-x86_64.snapshot").unwrap();
    let modified = format!("{}\n# DRIFT_TEST_MARKER=1\n", snap);
    let backup = std::fs::read("kernel/config-x86_64.snapshot").unwrap();
    std::fs::write("kernel/config-x86_64.snapshot", modified).unwrap();
    let result = Command::new(binary())
        .args(["kernel", "--force", "--output"])
        .arg(&out)
        .output()
        .unwrap();
    // Restore the snapshot regardless of test outcome.
    std::fs::write("kernel/config-x86_64.snapshot", backup).unwrap();

    assert!(!result.status.success(), "expected drift to fail");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(".config drift") || stderr.contains("update-snapshot"));
}

fn sha256(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).unwrap();
    hex::encode(h.finalize())
}
