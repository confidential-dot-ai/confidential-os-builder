use assert_cmd::Command;

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("build"))
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
fn test_build_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("output"))
        .stdout(predicates::str::contains("firmware"));
}

#[test]
fn test_build_skip_igvm_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
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

// --- push command tests ---

#[test]
fn test_push_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["push", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("registry"))
        .stdout(predicates::str::contains("tag"));
}

#[test]
fn test_push_requires_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["push"]).assert().failure();
}

// --- pull command tests ---

#[test]
fn test_pull_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["pull", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("registry"))
        .stdout(predicates::str::contains("tag"));
}

#[test]
fn test_pull_requires_name() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["pull"]).assert().failure();
}

// --- igvm command tests ---

#[test]
fn test_igvm_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["igvm", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("smp"))
        .stdout(predicates::str::contains("firmware"));
}

#[test]
fn test_igvm_requires_dir_and_smp() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["igvm"]).assert().failure();
}

// --- kernel command tests ---

#[test]
fn test_kernel_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
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
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--memory", "4GB", "--skip-igvm"])
        .assert()
        .failure();
}

#[test]
fn test_build_name_argument() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("[NAME]"))
        .stdout(predicates::str::contains("--smp"))
        .stdout(predicates::str::contains("--memory"));
}

#[test]
fn test_build_extra_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--extra"))
        .stdout(predicates::str::contains("-e"));
}

#[test]
fn test_build_package_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--package"))
        .stdout(predicates::str::contains("-p,"));
}

#[test]
fn test_build_script_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--script"))
        .stdout(predicates::str::contains("-s,"));
}
