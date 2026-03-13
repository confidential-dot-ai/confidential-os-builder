use assert_cmd::Command;

#[test]
fn test_help_shows_subcommands() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
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
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["cloud-init"])
        .assert()
        .failure();
}

#[test]
fn test_cloud_init_requires_kernel_flag() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["cloud-init", "/tmp/fake-dir", "--initrd", "/tmp/i", "--firmware", "/tmp/f", "--base-image", "/tmp/b", "-o", "/tmp/o"])
        .assert()
        .failure();
}

#[test]
fn test_container_requires_url() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["container"])
        .assert()
        .failure();
}

#[test]
fn test_base_requires_source_image() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["base", "-o", "/tmp/o"])
        .assert()
        .failure();
}

#[test]
fn test_kernel_requires_output() {
    let mut cmd = Command::cargo_bin("lunal-build").unwrap();
    cmd.args(["kernel", "--source", "/tmp/s", "--config", "/tmp/c"])
        .assert()
        .failure();
}
