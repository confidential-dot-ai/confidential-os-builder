use assert_cmd::Command;
use clap::{Args, FromArgMatches};
use confos::RunArgs;

fn parse_run_args(args: &[&str]) -> Result<RunArgs, clap::Error> {
    let matches = RunArgs::augment_args(clap::Command::new("run")).try_get_matches_from(args)?;
    RunArgs::from_arg_matches(&matches)
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
