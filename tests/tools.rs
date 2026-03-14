use steep::tools;

#[test]
fn test_require_finds_existing_tool() {
    // `sh` should always exist
    let result = tools::require("sh");
    assert!(result.is_ok());
}

#[test]
fn test_require_fails_for_missing_tool() {
    let result = tools::require("nonexistent-tool-xyz-123");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found in PATH"));
}

#[test]
fn test_run_command_success() {
    let output = tools::run_command("echo", &["hello"]).unwrap();
    assert_eq!(output.trim(), "hello");
}

#[test]
fn test_run_command_failure() {
    let result = tools::run_command("sh", &["-c", "exit 1"]);
    assert!(result.is_err());
}

#[test]
fn test_build_command_args() {
    let cmd = tools::CommandBuilder::new("igvm-tools")
        .arg("build")
        .arg_pair("--firmware", "/path/to/ovmf")
        .arg_pair("--kernel", "/path/to/uki")
        .arg_pair("--smp", "4")
        .arg_pair("--platform", "snp")
        .arg_pair("-o", "/path/to/output.igvm")
        .build();
    let args: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        args,
        vec![
            "build",
            "--firmware", "/path/to/ovmf",
            "--kernel", "/path/to/uki",
            "--smp", "4",
            "--platform", "snp",
            "-o", "/path/to/output.igvm",
        ]
    );
}
