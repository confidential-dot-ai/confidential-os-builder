use assert_cmd::Command;

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("seal"))
        .stdout(predicates::str::contains("run"));
}

#[test]
fn test_run_requires_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["run"]).assert().failure();
}

#[test]
fn test_run_accepts_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["run", "/tmp/nonexistent"]).assert().failure();
}

#[test]
fn test_help_shows_run_subcommand() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("run"));
}

#[test]
fn test_cloud_init_requires_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["cloud-init"]).assert().failure();
}

#[test]
fn test_seal_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["seal", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("output"))
        .stdout(predicates::str::contains("firmware"));
}

#[test]
fn test_seal_skip_igvm_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["seal", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("skip-igvm"))
        .stdout(predicates::str::contains("cloud-init"));
}

#[test]
fn test_run_port_forward_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("port-forward"));
}
