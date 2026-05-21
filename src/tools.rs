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
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
/// Uses OsStr args to avoid lossy UTF-8 conversion corrupting paths.
pub fn sudo_mv(src: &Path, dst: &Path) -> Result<(), ToolError> {
    run_command_streaming(
        "sudo",
        &[OsStr::new("mv"), src.as_os_str(), dst.as_os_str()],
    )?;
    // RUNTIME lookup — `std::env!("USER")` (the macro) would bake the
    // build-host's username into the binary, which then fails on any other
    // machine where that username doesn't exist. Prefer SUDO_USER (when
    // the caller invoked us through sudo) so we hand ownership back to the
    // original user rather than root; fall back to USER for the common
    // case where the user runs steep directly and steep sudo's internally;
    // and fall back to "root" if neither is set (file stays root-owned —
    // the operator can chown it after, no failure).
    let user = std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "root".to_string());
    run_command_streaming(
        "sudo",
        &[
            OsStr::new("chown"),
            OsStr::new(&user),
            dst.as_os_str(),
        ],
    )?;
    Ok(())
}

/// Make a root-owned file readable (chmod 644 via sudo).
/// Uses OsStr args to avoid lossy UTF-8 conversion corrupting paths.
pub fn sudo_chmod_readable(path: &Path) -> Result<(), ToolError> {
    run_command_streaming(
        "sudo",
        &[OsStr::new("chmod"), OsStr::new("644"), path.as_os_str()],
    )?;
    Ok(())
}

/// Recursively remove a directory tree, escalating to `sudo rm -rf` on EPERM.
///
/// Kernel build steps run inside `systemd-nspawn` as root and bind-mount host
/// directories, so the nspawn writes files the host user can't later delete.
/// Without this fallback, a single interrupted (or completed) build leaves
/// `output/kernel/build` un-wipeable and every subsequent run fails at the
/// "extract + configure" step. Trying the plain remove first keeps the no-sudo
/// path fast for clean trees.
pub fn force_remove_dir_all(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match fs_err::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                path = %path.display(),
                "permission denied removing tree; retrying with sudo (likely root-owned files left by nspawn)"
            );
            let status = Command::new("sudo")
                .arg("rm")
                .arg("-rf")
                .arg("--")
                .arg(path)
                .status()?;
            if !status.success() {
                return Err(std::io::Error::other(format!(
                    "sudo rm -rf {} failed (exit {:?})",
                    path.display(),
                    status.code()
                )));
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Resolve the canonical path of mkosi, following symlinks.
/// uv-installed mkosi lives at ~/.local/bin/mkosi -> ~/.local/share/uv/tools/mkosi/bin/mkosi
/// which has a shebang pointing to the venv Python. sudo + env + PATH can't resolve
/// through this chain, so we resolve it once and invoke the full path directly.
pub fn resolve_mkosi() -> Result<String, ToolError> {
    let path = require("mkosi")?;
    path.canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| ToolError::Io {
            tool: "mkosi".to_string(),
            source: e,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn force_remove_dir_all_removes_user_owned_tree() {
        let d = TempDir::new().unwrap();
        let inner = d.path().join("a/b/c");
        fs_err::create_dir_all(&inner).unwrap();
        fs_err::write(inner.join("f"), b"hi").unwrap();
        force_remove_dir_all(d.path()).unwrap();
        assert!(!d.path().exists());
    }

    #[test]
    fn force_remove_dir_all_is_a_noop_for_missing_path() {
        let d = TempDir::new().unwrap();
        let missing = d.path().join("does-not-exist");
        force_remove_dir_all(&missing).unwrap();
    }
}
