use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("{tool} not found in PATH. Install it and try again.")]
    NotFound { tool: String },

    #[error("{tool} failed with exit code {code}:\n{stderr}")]
    Failed {
        tool: String,
        code: i32,
        stderr: String,
    },

    #[error("{tool} was terminated by a signal")]
    Signal { tool: String },

    #[error("failed to execute {tool}: {source}")]
    Io {
        tool: String,
        source: std::io::Error,
    },
}

/// Check that an external tool is available in PATH.
pub fn require(tool: &str) -> Result<PathBuf, ToolError> {
    which::which(tool).map_err(|_| ToolError::NotFound {
        tool: tool.to_string(),
    })
}

/// Run a command and return its stdout as a string.
/// Fails if the command exits with a non-zero status.
pub fn run_command(tool: &str, args: &[&str]) -> Result<String, ToolError> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(|e| ToolError::Io {
            tool: tool.to_string(),
            source: e,
        })?;

    if !output.status.success() {
        let code = output.status.code().ok_or(ToolError::Signal {
            tool: tool.to_string(),
        })?;
        return Err(ToolError::Failed {
            tool: tool.to_string(),
            code,
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a command with inherited stdio (streams output to the terminal).
/// Fails if the command exits with a non-zero status.
pub fn run_command_streaming(tool: &str, args: &[impl AsRef<OsStr>]) -> Result<(), ToolError> {
    let cwd = std::env::current_dir().map_err(|e| ToolError::Io {
        tool: tool.to_string(),
        source: e,
    })?;
    run_command_streaming_in(tool, args, cwd)
}

pub fn run_command_streaming_in(
    tool: &str,
    args: &[impl AsRef<OsStr>],
    cwd: PathBuf,
) -> Result<(), ToolError> {
    tracing::debug!(
        cmd = %format!("{} {}", tool, args.iter().map(|i| i.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" ")),
        "exec"
    );
    let status = Command::new(tool)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| ToolError::Io {
            tool: tool.to_string(),
            source: e,
        })?;

    if !status.success() {
        let code = status.code().ok_or(ToolError::Signal {
            tool: tool.to_string(),
        })?;
        return Err(ToolError::Failed {
            tool: tool.to_string(),
            code,
            stderr: String::new(),
        });
    }

    Ok(())
}

/// Copy a file with sudo and set permissions to 644.
/// mkosi outputs are root-owned; this copies them to the output directory readably.
pub fn sudo_copy(src: &Path, dst: &Path) -> Result<(), ToolError> {
    run_command("sudo", &["cp", &src.to_string_lossy(), &dst.to_string_lossy()])?;
    run_command("sudo", &["chmod", "644", &dst.to_string_lossy()])?;
    Ok(())
}

/// Make a root-owned file readable (chmod 644 via sudo).
pub fn sudo_chmod_readable(path: &Path) -> Result<(), ToolError> {
    run_command("sudo", &["chmod", "644", &path.to_string_lossy()])?;
    Ok(())
}

/// Safe PATH for sudo commands — only system directories, no user-controlled paths.
/// Includes ~/.local/bin only if mkosi was installed there (pip install --user).
pub fn safe_path() -> String {
    let base = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
    if let Ok(home) = std::env::var("HOME") {
        let local_bin = format!("{home}/.local/bin");
        if Path::new(&local_bin).join("mkosi").exists() {
            return format!("{local_bin}:{base}");
        }
    }
    base.to_string()
}

