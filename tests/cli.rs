use assert_cmd::Command;
use clap::{Args, FromArgMatches};
use confos::RunArgs;
use std::os::unix::fs::PermissionsExt;

fn run_output(smp_counts: &[u32]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let manifest = serde_json::json!({
        "version": confos::manifest::MANIFEST_VERSION,
        "build": {"timestamp": "2026-01-01T00:00:00Z", "memory": "2G", "format": "raw", "platform": "snp"},
        "inputs": {
            "initrd": {"path": "initrd.cpio.gz", "sha256": ""},
            "firmware": {"path": "OVMF.fd", "sha256": ""},
            "base_image": {"path": "base.raw", "sha256": ""}
        },
        "outputs": {
            "disk_image": {"path": "disk.raw", "sha256": ""},
            "uki": {"path": "uki.efi", "sha256": ""}
        },
        "snp_variants": smp_counts.iter().map(|smp| serde_json::json!({
            "smp": smp,
            "igvm": {"path": format!("guest-smp{smp}.igvm"), "sha256": ""},
            "measurement": {"snp_launch_digest": format!("digest{smp}"), "algorithm": "sha384", "page_count": 1, "vmsa_count": smp}
        })).collect::<Vec<_>>()
    });
    std::fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
    for artifact in ["disk.raw", "uki.efi", "OVMF.fd"] {
        std::fs::write(dir.path().join(artifact), []).unwrap();
    }
    dir
}

fn non_snp_qemu_stub(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("qemu-stub");
    std::fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = "-object" ] && [ "$2" = "help" ]; then
    exit 0
fi
printf '%s\n' "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn test_run_smp_override_on_fallback_tier() {
    for counts in [vec![], vec![2, 4, 8, 16]] {
        let dir = run_output(&counts);
        Command::cargo_bin("confos")
            .unwrap()
            .arg("run")
            .arg(dir.path())
            .arg("--qemu-bin")
            .arg(non_snp_qemu_stub(dir.path()))
            .args(["--smp", "3"])
            .assert()
            .success()
            .stdout(predicates::str::contains("-smp\n3\n"))
            .stdout(predicates::str::contains("Launch digest:").count(0));
    }
}

#[test]
fn test_run_default_smp_on_fallback_tier() {
    for (counts, expected) in [(vec![], "2"), (vec![4, 2, 8, 16], "4")] {
        let dir = run_output(&counts);
        Command::cargo_bin("confos")
            .unwrap()
            .arg("run")
            .arg(dir.path())
            .arg("--qemu-bin")
            .arg(non_snp_qemu_stub(dir.path()))
            .assert()
            .success()
            .stdout(predicates::str::contains(format!("-smp\n{expected}\n")))
            .stdout(predicates::str::contains("Launch digest:").count(0));
    }
}

#[test]
fn test_run_smp_parser_range() {
    for valid in ["1", "8", "1024"] {
        assert_eq!(
            parse_run_args(&["run", "--smp", valid]).unwrap().smp,
            Some(valid.parse().unwrap())
        );
    }
    for invalid in ["0", "1025", "-1", "not-a-number"] {
        assert!(parse_run_args(&["run", "--smp", invalid]).is_err());
    }
}

#[test]
fn test_run_help_shows_smp() {
    Command::cargo_bin("confos")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--smp"));
}

fn parse_run_args(args: &[&str]) -> Result<RunArgs, clap::Error> {
    let matches = RunArgs::augment_args(clap::Command::new("run")).try_get_matches_from(args)?;
    RunArgs::from_arg_matches(&matches)
}

#[test]
fn test_run_cdrom_is_repeatable() {
    let args = parse_run_args(&["run", "--cdrom", "op.iso", "--cdrom", "cidata.iso"]).unwrap();
    assert_eq!(
        args.cdroms,
        [
            std::path::PathBuf::from("op.iso"),
            std::path::PathBuf::from("cidata.iso")
        ]
    );
}

#[test]
fn test_run_help_shows_cdrom() {
    Command::cargo_bin("confos")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--cdrom"));
}

#[test]
fn test_run_missing_cdrom_fails_before_qemu_probe() {
    let dir = run_output(&[]);
    let media = dir.path().join("missing.iso");
    Command::cargo_bin("confos")
        .unwrap()
        .arg("run")
        .arg(dir.path())
        .arg("--cdrom")
        .arg(&media)
        .args(["--qemu-bin", "/nonexistent/qemu"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "cdrom must be an existing regular file",
        ));
}

#[test]
fn test_run_host_data_parser() {
    let args = parse_run_args(&["run", "--host-data", &"ab".repeat(32)]).unwrap();
    assert_eq!(args.host_data, Some([0xab; 32]));
    for invalid in [
        "a".repeat(63),
        "a".repeat(65),
        "AB".repeat(32),
        "g".repeat(64),
        "é".repeat(32),
    ] {
        let err = parse_run_args(&["run", "--host-data", &invalid])
            .err()
            .unwrap();
        assert!(err
            .to_string()
            .contains("exactly 64 lowercase hex characters"));
    }
}

#[test]
fn test_run_help_shows_host_data() {
    Command::cargo_bin("confos")
        .unwrap()
        .args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--host-data"));
}

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("build"))
        .stdout(predicates::str::contains("run"));
}

#[test]
fn test_run_requires_dir() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["run"]).assert().failure();
}

#[test]
fn test_run_accepts_dir() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["run", "/tmp/nonexistent"]).assert().failure();
}

#[test]
fn test_help_shows_run_subcommand() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("run"));
}

#[test]
fn test_cloud_init_requires_dir() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["cloud-init"]).assert().failure();
}

#[test]
fn test_build_help() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("output"))
        .stdout(predicates::str::contains("firmware"));
}

#[test]
fn test_build_skip_igvm_flag() {
    // --skip-igvm survives as a deprecated alias for --platform tdx;
    // verify it still shows up in --help.
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("skip-igvm"))
        .stdout(predicates::str::contains("cloud-init"));
}

#[test]
fn test_build_platform_flag_documented() {
    // --platform replaces --skip-igvm; check it documents the three
    // expected values.
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--platform"))
        .stdout(predicates::str::contains("snp"))
        .stdout(predicates::str::contains("tdx"))
        .stdout(predicates::str::contains("both"));
}

#[test]
fn test_build_rejects_skip_igvm_with_explicit_platform() {
    // The deprecated alias is rejected when combined with an explicit
    // --platform (other than the default) so silent ambiguity is
    // impossible. We exercise the --help path indirectly here — the
    // actual rejection happens at run-time in commands::build::run, so
    // assert it via a build invocation that fails fast on mkosi.
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--platform", "snp", "--skip-igvm"])
        .assert()
        .failure();
}

#[test]
fn test_run_port_forward_flag() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("port-forward"));
}

#[test]
fn test_run_help_shows_scratch() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--scratch"));
}

// --- push command tests ---

#[test]
fn test_push_help() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["push", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("registry"))
        .stdout(predicates::str::contains("tag"));
}

#[test]
fn test_push_requires_dir() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["push"]).assert().failure();
}

// --- pull command tests ---

#[test]
fn test_pull_help() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["pull", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("registry"))
        .stdout(predicates::str::contains("tag"));
}

#[test]
fn test_pull_requires_name() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["pull"]).assert().failure();
}

// --- igvm command tests ---

#[test]
fn test_igvm_help() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["igvm", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("smp"))
        .stdout(predicates::str::contains("firmware"));
}

#[test]
fn test_igvm_requires_dir_and_smp() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["igvm"]).assert().failure();
}

// --- kernel command tests ---

#[test]
fn test_kernel_help() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["kernel", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("force"))
        .stdout(predicates::str::contains("kernel-config-fragment"))
        .stdout(predicates::str::contains("output"));
}

// --- build command validation tests ---

#[test]
fn test_build_rejects_invalid_memory() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--memory", "4GB", "--skip-igvm"])
        .assert()
        .failure();
}

#[test]
fn test_build_name_argument() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[NAME]"))
        .stdout(predicates::str::contains("--smp"))
        .stdout(predicates::str::contains("--memory"));
}

#[test]
fn test_build_extra_flag() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--extra"))
        .stdout(predicates::str::contains("-e"));
}

#[test]
fn test_build_package_flag() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--package"))
        .stdout(predicates::str::contains("-p,"));
}

#[test]
fn test_build_script_flag() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--script"))
        .stdout(predicates::str::contains("-s,"));
}

#[test]
fn test_build_out_of_tree_profile_flags() {
    let mut cmd = Command::cargo_bin("confos").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--profile-dir"))
        .stdout(predicates::str::contains("<DIR>"))
        .stdout(predicates::str::contains("--sync-input"))
        .stdout(predicates::str::contains("<NAME=VALUE>"));
}
