use std::ffi::OsStr;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
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

pub fn run_command_exec(tool: &str, args: &[&str]) -> Result<(), ToolError> {
    println!(
        "🍵 {} {}",
        tool,
        args.iter()
            .map(|i| i.to_owned())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let _ = Command::new(tool).args(args).exec();

    Ok(())
}

/// Run a command with inherited stdio (streams output to the terminal).
/// Fails if the command exits with a non-zero status.
pub fn run_command_streaming(tool: &str, args: &[impl AsRef<OsStr>]) -> Result<(), ToolError> {
    run_command_streaming_in(tool, args, std::env::current_dir().unwrap())
}

pub fn run_command_streaming_in(
    tool: &str,
    args: &[impl AsRef<OsStr>],
    cwd: PathBuf,
) -> Result<(), ToolError> {
    println!(
        "🍵 {} {}",
        tool,
        args.iter()
            .map(|i| i.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
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

/// Builder for constructing command argument lists.
pub struct CommandBuilder {
    tool: String,
    args: Vec<String>,
}

impl CommandBuilder {
    pub fn new(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn arg_pair(mut self, flag: &str, value: &str) -> Self {
        self.args.push(flag.to_string());
        self.args.push(value.to_string());
        self
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    pub fn build(self) -> Vec<String> {
        self.args
    }
}
