//! Kernel `.config` resolution and snapshot guard.
//!
//! The "configure phase" runs `make x86_64_defconfig`, applies fragments,
//! then `mod2yesconfig`, then `olddefconfig`. After that, the resolved
//! `.config` is compared against the committed snapshot via [`check_snapshot`].

use std::ffi::OsString;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::tools;

/// Read two files and assert byte-equality. Returns Ok(()) on match.
/// On mismatch, error includes both paths so the caller can suggest a diff.
pub fn check_snapshot(resolved: &Path, snapshot: &Path) -> Result<()> {
    let a = fs_err::read(resolved)
        .with_context(|| format!("reading resolved config {}", resolved.display()))?;
    let b = fs_err::read(snapshot)
        .with_context(|| format!("reading snapshot {}", snapshot.display()))?;
    if a == b {
        Ok(())
    } else {
        Err(anyhow!(
            "kernel .config drift: {} differs from {}.\n\
             Review the diff and re-run with `steep kernel --update-snapshot` if intended.",
            resolved.display(),
            snapshot.display()
        ))
    }
}

/// Replace `snapshot` with the contents of `resolved`. Used by --update-snapshot.
pub fn update_snapshot(resolved: &Path, snapshot: &Path) -> Result<()> {
    let bytes = fs_err::read(resolved)
        .with_context(|| format!("reading resolved config {}", resolved.display()))?;
    fs_err::write(snapshot, bytes)
        .with_context(|| format!("writing snapshot {}", snapshot.display()))?;
    Ok(())
}

/// Orchestrate the configure phase inside systemd-nspawn against the kernel-builder tools tree.
///
/// Inside the tools tree, runs (in this order):
///   make x86_64_defconfig
///   scripts/kconfig/merge_config.sh -m .config <required>
///   scripts/kconfig/merge_config.sh -m .config <hardening>
///   scripts/kconfig/merge_config.sh -m .config <container>   (when Some)
///   make mod2yesconfig
///   make olddefconfig
///
/// `container_fragment` is optional. When `Some`, it's merged after
/// `hardening.config` so `mod2yesconfig` still flattens any tristate
/// symbols introduced. When `None` the merge sequence is byte-for-byte
/// what it was before this fragment was added — kept this way so the
/// kernel snapshot for callers that don't supply a container fragment
/// is unchanged.
pub fn run_configure_phase(
    tools_tree: &Path,
    kernel_dir: &Path,
    required_fragment: &Path,
    hardening_fragment: &Path,
    container_fragment: Option<&Path>,
) -> Result<()> {
    let kernel_dir_abs = kernel_dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", kernel_dir.display()))?;
    let required_abs = required_fragment.canonicalize()?;
    let hardening_abs = hardening_fragment.canonicalize()?;
    let container_abs = container_fragment.map(|p| p.canonicalize()).transpose()?;

    // Stage fragments inside the kernel dir so merge_config can find them
    // at relative paths under /build inside the nspawn.
    let frag_dir_in_kernel = kernel_dir_abs.join(".fragments");
    fs_err::create_dir_all(&frag_dir_in_kernel)?;
    fs_err::copy(&required_abs, frag_dir_in_kernel.join("required.config"))?;
    fs_err::copy(&hardening_abs, frag_dir_in_kernel.join("hardening.config"))?;
    if let Some(ref c) = container_abs {
        fs_err::copy(c, frag_dir_in_kernel.join("container.config"))?;
    }

    let container_line = if container_abs.is_some() {
        "scripts/kconfig/merge_config.sh -m .config .fragments/container.config\n"
    } else {
        ""
    };
    let script = format!(
        "set -eux\n\
         cd /build\n\
         make x86_64_defconfig\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/required.config\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/hardening.config\n\
         {container_line}\
         make mod2yesconfig\n\
         make olddefconfig\n",
    );

    nspawn(
        tools_tree,
        &kernel_dir_abs,
        "/build",
        &[("HOME", "/root")],
        &script,
    )?;
    fs_err::remove_dir_all(&frag_dir_in_kernel)?;
    Ok(())
}

/// Run a shell script inside `tools_tree` with `host_dir` bind-mounted at `mount_at`.
/// `env_vars` is `(name, value)` pairs forwarded via `--setenv`.
pub fn nspawn(
    tools_tree: &Path,
    host_dir: &Path,
    mount_at: &str,
    env_vars: &[(&str, &str)],
    script: &str,
) -> Result<()> {
    let nspawn_bin = tools::require("systemd-nspawn")
        .map_err(|_| anyhow!("systemd-nspawn required; install systemd-container"))?;

    let mut args: Vec<OsString> = vec![
        OsString::from("--quiet"),
        OsString::from("--register=no"),
        OsString::from("--keep-unit"),
        OsString::from("--ephemeral"),
        OsString::from("--directory"),
        tools_tree.into(),
        OsString::from("--bind"),
        OsString::from(format!("{}:{}", host_dir.display(), mount_at)),
    ];
    for (k, v) in env_vars {
        args.push(OsString::from("--setenv"));
        args.push(OsString::from(format!("{}={}", k, v)));
    }
    args.push(OsString::from("/bin/bash"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(script));

    // CRITICAL: build the full sudo args vec in a let-binding before passing
    // a slice into run_command_streaming. Earlier drafts inlined this with
    // `{ let mut v = ...; &v[..] }` which dangled.
    let mut v: Vec<OsString> = vec![nspawn_bin.as_os_str().into()];
    v.extend(args);
    tools::run_command_streaming("sudo", &v[..]).map_err(|e| anyhow!("nspawn failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs_err::write(&p, content).unwrap();
        p
    }

    #[test]
    fn check_snapshot_passes_on_match() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=y\n");
        assert!(check_snapshot(&a, &b).is_ok());
    }

    #[test]
    fn check_snapshot_fails_on_diff_with_helpful_message() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=n\n");
        let err = check_snapshot(&a, &b).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(".config drift"));
        assert!(msg.contains("--update-snapshot"));
    }

    #[test]
    fn update_snapshot_overwrites_target() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=n\n");
        update_snapshot(&a, &b).unwrap();
        assert_eq!(fs_err::read_to_string(&b).unwrap(), "CONFIG_X=y\n");
    }

    #[test]
    fn check_snapshot_errors_on_missing_file() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "x");
        let b = d.path().join("does-not-exist");
        assert!(check_snapshot(&a, &b).is_err());
    }
}
