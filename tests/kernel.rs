//! Integration tests for steep kernel + kernel_cache.
//!
//! These run a real kernel build under systemd-nspawn. Mark with `#[ignore]`
//! so `cargo test` doesn't trigger a 10+ minute build.
//!
//! Run with one of:
//!   - `cargo nextest run --run-ignored only` — `.config/nextest.toml`
//!     serializes this binary onto a single thread.
//!   - `cargo test --test kernel -- --ignored --test-threads=1` — plain cargo
//!     does not honor nextest config, so the flag is required.
//!
//! These tests share project-relative state (`mkosi/kernel-builder/` tools
//! tree rebuilt by `mkosi --force`, and `kernel/config-x86_64.snapshot`
//! rewritten by every kernel build) and fail when run in parallel.
//!
//! Requires:
//!   - `kernel/version`, `kernel/required.config`, `kernel/hardening.config`
//!     checked in
//!   - `mkosi/kernel-builder/` config in place
//!   - sudo + systemd-nspawn available
//!   - network access to cdn.kernel.org
//!
//! Each test runs in its own temp output dir (no interference with `output/kernel/`).

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("steep")
}

/// Tempdir that reclaims root ownership (left by `sudo systemd-nspawn` during
/// the build) before the inner `TempDir`'s `Drop` rms it. Without this, every
/// test run leaks ~2 GB of root-owned kernel source/objects in tmpfs because
/// `remove_dir_all` runs as the test user and silently fails on root files.
struct KernelOut(tempfile::TempDir);

impl KernelOut {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for KernelOut {
    fn drop(&mut self) {
        let user = std::env::var("USER").unwrap_or_else(|_| "root".into());
        let _ = std::process::Command::new("sudo")
            .args(["chown", "-R", &user])
            .arg(self.0.path())
            .status();
    }
}

#[test]
#[ignore]
fn kernel_build_succeeds() {
    let tmp = KernelOut::new();
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
    // Build sequentially, dropping each tempdir between builds so peak disk
    // usage is one build tree (~2 GB on tmpfs), not two.
    let h1 = build_and_hash_vmlinuz();
    let h2 = build_and_hash_vmlinuz();
    assert_eq!(h1, h2, "vmlinuz not reproducible across builds");
}

fn build_and_hash_vmlinuz() -> String {
    let tmp = KernelOut::new();
    let out = tmp.path().join("kernel");
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();
    sha256(&out.join("vmlinuz"))
}

#[test]
#[ignore]
fn kernel_cache_hits_on_second_run() {
    let tmp = KernelOut::new();
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
fn kernel_build_rewrites_snapshot() {
    // The snapshot is an auto-updating lockfile: a build always rewrites it
    // with the freshly-resolved `.config` and never fails on drift. Mutating
    // the snapshot beforehand must not break the build, and the injected
    // marker must be gone afterwards.
    let tmp = KernelOut::new();
    let out = tmp.path().join("kernel");

    let snapshot = "kernel/config-x86_64.snapshot";
    let backup = std::fs::read(snapshot).unwrap();
    let mutated = format!(
        "{}\n# DRIFT_TEST_MARKER=1\n",
        String::from_utf8_lossy(&backup)
    );
    std::fs::write(snapshot, mutated).unwrap();

    let result = Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .output()
        .unwrap();

    let after = std::fs::read_to_string(snapshot).unwrap_or_default();
    // Restore the snapshot regardless of test outcome.
    std::fs::write(snapshot, &backup).unwrap();

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

    assert!(
        result.status.success(),
        "expected build to succeed despite snapshot drift. stdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        !after.contains("DRIFT_TEST_MARKER"),
        "snapshot was not rewritten — marker still present. stdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

fn sha256(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).unwrap();
    hex::encode(h.finalize())
}
