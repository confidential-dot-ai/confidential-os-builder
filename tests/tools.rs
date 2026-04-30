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
fn test_run_command_failure_contains_exit_code() {
    let result = tools::run_command("sh", &["-c", "exit 42"]);
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("42"),
        "error should contain exit code: {err}"
    );
}

#[test]
fn test_run_command_captures_stderr() {
    let result = tools::run_command("sh", &["-c", "echo oops >&2; exit 1"]);
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("oops"),
        "error should contain stderr: {err}"
    );
}

#[test]
fn test_run_command_nonexistent_binary() {
    let result = tools::run_command("nonexistent-binary-xyz", &[]);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("nonexistent-binary-xyz"));
}

#[test]
fn test_run_command_streaming_success() {
    let result = tools::run_command_streaming("sh", &["-c", "true"]);
    assert!(result.is_ok());
}

#[test]
fn test_run_command_streaming_failure() {
    let result = tools::run_command_streaming("sh", &["-c", "exit 1"]);
    assert!(result.is_err());
}

#[test]
fn test_run_command_streaming_nonexistent_binary() {
    let result = tools::run_command_streaming("nonexistent-binary-xyz", &["-c", "true"]);
    assert!(result.is_err());
}

#[test]
fn test_run_command_streaming_in_with_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let result = tools::run_command_streaming_in("sh", &["-c", "true"], dir.path().to_path_buf());
    assert!(result.is_ok());
}

#[test]
fn test_tool_error_not_found_display() {
    let err = tools::require("nonexistent-tool-xyz-123").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nonexistent-tool-xyz-123"));
    assert!(msg.contains("not found in PATH"));
}

#[test]
fn test_tool_error_failed_display() {
    let err = tools::run_command("sh", &["-c", "echo bad >&2; exit 7"]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("7"), "should contain exit code: {msg}");
    assert!(msg.contains("bad"), "should contain stderr: {msg}");
}
