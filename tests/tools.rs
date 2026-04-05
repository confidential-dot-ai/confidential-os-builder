use steep::tools;

#[test]
fn test_require_finds_existing_tool() {
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
fn test_safe_path_includes_system_dirs() {
    let path = tools::safe_path();
    assert!(path.contains("/usr/bin"));
    assert!(path.contains("/usr/sbin"));
}
