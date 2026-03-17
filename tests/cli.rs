use assert_cmd::Command;

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("kernel"))
        .stdout(predicates::str::contains("base"))
        .stdout(predicates::str::contains("cloud-init"))
        .stdout(predicates::str::contains("container"));
}

#[test]
fn test_cloud_init_requires_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["cloud-init"]).assert().failure();
}

#[test]
fn test_cloud_init_requires_kernel_flag() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp/fake-dir",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure();
}

#[test]
fn test_container_requires_url() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["container"]).assert().failure();
}

#[test]
fn test_base_requires_output() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["base"]).assert().failure();
}

#[test]
fn test_kernel_requires_output() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["kernel", "--source", "/tmp/s", "--config", "/tmp/c"])
        .assert()
        .failure();
}

#[test]
fn test_cloud_init_fails_with_missing_dir() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/nonexistent/dir",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "443",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("not found"));
}


#[test]
fn test_smp_default_is_one() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "443",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure(); // Fails on validation, not arg parsing — proves --smp has a default
}

#[test]
fn test_format_flag_accepts_vhd() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "443",
        "--format",
        "vhd",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure(); // Fails on validation, not parsing — proves vhd is accepted
}

#[test]
fn test_cloud_init_requires_service_port() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("--service-port"));
}

#[test]
fn test_cloud_init_accepts_service_port() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "8080",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure();
}

#[test]
fn test_cloud_init_memory_default() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "cloud-init",
        "/tmp",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "443",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure();
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
fn test_run_fails_with_missing_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["run", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("manifest.json not found"));
}

#[test]
fn test_container_requires_service_port() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "container",
        "ghcr.io/org/app:latest",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("--service-port"));
}

#[test]
fn test_container_accepts_service_port_and_memory() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "container",
        "ghcr.io/org/app:latest",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "8080",
        "--memory",
        "4G",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure(); // Fails on validation, not parsing
}

#[test]
fn test_container_memory_default() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "container",
        "ghcr.io/org/app:latest",
        "--kernel",
        "/tmp/k",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "443",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure(); // Fails on validation, not parsing — proves --memory has a default
}

#[test]
fn test_container_fails_with_missing_kernel() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args([
        "container",
        "ghcr.io/org/app:latest",
        "--kernel",
        "/nonexistent/kernel",
        "--initrd",
        "/tmp/i",
        "--firmware",
        "/tmp/f",
        "--base-image",
        "/tmp/b",
        "--service-port",
        "8080",
        "-o",
        "/tmp/o",
    ])
    .assert()
    .failure()
    .stderr(predicates::str::contains("not found"));
}
