use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
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

    #[error(
        "{tool} fetched from live (unpinned) Ubuntu mirrors — the build is not \
         reproducible and the measurement drifts with the archive (#96):\n{lines}\n\
         Every apt source must point at snapshot.ubuntu.com by URI: provide the \
         full deb822 list in mkosi.sandbox/etc/apt/sources.list.d/mkosi.sources \
         and hand the same tree to the tools tree via ToolsTreeSandboxTrees= \
         (mkosi's own Mirror=/Snapshot= both leave live pockets reachable)."
    )]
    LiveMirror { tool: String, lines: String },
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

/// An apt progress line fetching from a live (unpinned) Ubuntu mirror, or None.
///
/// apt prints one line per index/package fetch: `Get:12 http://host/path suite ...`
/// (also Hit/Ign/Err). When every pocket is pinned, apt rewrites all of them to
/// snapshot.ubuntu.com — so any other *.ubuntu.com archive host in a fetch line
/// means a pocket escaped the pin (#96). Matching only real fetch-progress lines
/// keeps config echoes (a deb822 URIs: line names the live host by design) from
/// tripping the guard.
fn live_mirror_fetch(line: &str) -> Option<&str> {
    let mut parts = line.split_whitespace();
    let tag = parts.next()?;
    let (kind, seq) = tag.split_once(':')?;
    if !matches!(kind, "Get" | "Hit" | "Ign" | "Err") {
        return None;
    }
    if seq.is_empty() || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let url = parts.next()?;
    let host = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?
        .split('/')
        .next()?;
    let live = matches!(
        host,
        "archive.ubuntu.com" | "security.ubuntu.com" | "ports.ubuntu.com" | "esm.ubuntu.com"
    ) || host.ends_with(".archive.ubuntu.com");
    live.then_some(host)
}

/// Run a command with output streamed to the terminal AND scanned for apt
/// fetches from live Ubuntu mirrors; fail if any occurred, even on exit 0.
///
/// This is the fail-closed half of the snapshot pinning: config presence is
/// not proof (mkosi's Snapshot= silently never reached the tools tree, and an
/// apt without deb822 Snapshot support silently ignores the field — both
/// fail open, #96). The apt log is the ground truth, so assert on it.
pub fn run_command_streaming_mirror_guarded(
    tool: &str,
    args: &[impl AsRef<OsStr>],
) -> Result<(), ToolError> {
    let io_err = |e: std::io::Error| ToolError::Io {
        tool: tool.to_string(),
        source: e,
    };
    tracing::debug!(
        cmd = %format!("{} {}", tool, args.iter().map(|i| i.as_ref().to_string_lossy()).collect::<Vec<_>>().join(" ")),
        "exec (mirror-guarded)"
    );
    let mut child = Command::new(tool)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(io_err)?;

    // Tee both streams line-wise; collect offending lines. read_until (not
    // lines()) so a non-UTF-8 byte cannot end the scan early — that would
    // fail open for everything after it.
    fn tee_scan(reader: impl Read, mut sink: impl Write, hits: &mut Vec<String>) {
        let mut buf = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match buf.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let _ = sink.write_all(&line);
            let text = String::from_utf8_lossy(&line);
            if live_mirror_fetch(text.trim_start_matches(['\r', ' '])).is_some() {
                hits.push(text.trim_end().to_string());
            }
        }
    }

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (mut out_hits, mut err_hits) = (Vec::new(), Vec::new());
    std::thread::scope(|s| {
        s.spawn(|| tee_scan(stdout, std::io::stdout(), &mut out_hits));
        s.spawn(|| tee_scan(stderr, std::io::stderr(), &mut err_hits));
    });
    let status = child.wait().map_err(io_err)?;

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
    out_hits.append(&mut err_hits);
    if !out_hits.is_empty() {
        out_hits.sort();
        out_hits.dedup();
        return Err(ToolError::LiveMirror {
            tool: tool.to_string(),
            lines: out_hits.join("\n"),
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
    // case where the user runs confos directly and confos sudo's internally;
    // and fall back to "root" if neither is set (file stays root-owned —
    // the operator can chown it after, no failure).
    let user = std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "root".to_string());
    run_command_streaming(
        "sudo",
        &[OsStr::new("chown"), OsStr::new(&user), dst.as_os_str()],
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

    #[test]
    fn live_mirror_fetch_flags_live_hosts_in_fetch_lines() {
        for line in [
            "Get:1 http://archive.ubuntu.com/ubuntu resolute InRelease [136 kB]",
            "Hit:3 http://security.ubuntu.com/ubuntu resolute-security InRelease",
            "Err:12 https://ports.ubuntu.com resolute/main arm64 Packages",
            "Get:7 http://azure.archive.ubuntu.com/ubuntu resolute/main amd64 tar 1.35 [258 kB]",
            "Ign:2 https://esm.ubuntu.com/apps/ubuntu resolute-apps-security InRelease",
        ] {
            assert!(live_mirror_fetch(line).is_some(), "{line}");
        }
    }

    #[test]
    fn live_mirror_fetch_ignores_pinned_and_non_fetch_lines() {
        for line in [
            // The pin working as intended.
            "Get:1 https://snapshot.ubuntu.com/ubuntu/20260405T000000Z/dists/resolute/InRelease [136 kB]",
            // deb822 config echo names the live host by design; not a fetch.
            "URIs: http://archive.ubuntu.com/ubuntu",
            // Prose mentioning a live host.
            "mkosi: falling back to archive.ubuntu.com",
            // Malformed sequence number — not an apt progress line.
            "Get:x http://archive.ubuntu.com/ubuntu resolute InRelease",
            "Get:1 http://packages.microsoft.com/repos/azure-cli resolute InRelease",
        ] {
            assert!(live_mirror_fetch(line).is_none(), "{line}");
        }
    }
}
