//! Kernel `.config` resolution and snapshot lockfile.
//!
//! The "configure phase" runs `make x86_64_defconfig`, applies fragments,
//! then `mod2yesconfig`, then `olddefconfig`. The resolved `.config` is
//! then written back to the committed snapshot via [`update_snapshot`],
//! which steep tracks in git like a lockfile.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::tools;

/// Overwrite `snapshot` with the freshly-resolved `.config`, returning
/// whether the content changed (`true` if the snapshot was absent or
/// differed). steep calls this on every kernel build, so the snapshot
/// tracks the resolved config like a lockfile; `git diff` on the snapshot
/// is what surfaces unexpected drift — the build itself never fails on it.
pub fn update_snapshot(resolved: &Path, snapshot: &Path) -> Result<bool> {
    let new = fs_err::read(resolved)
        .with_context(|| format!("reading resolved config {}", resolved.display()))?;
    let changed = match fs_err::read(snapshot) {
        Ok(old) => old != new,
        Err(_) => true,
    };
    fs_err::write(snapshot, &new)
        .with_context(|| format!("writing snapshot {}", snapshot.display()))?;
    Ok(changed)
}

/// Orchestrate the configure phase inside systemd-nspawn against the kernel-builder tools tree.
///
/// Inside the tools tree, runs (in this order):
///   make x86_64_defconfig
///   scripts/kconfig/merge_config.sh -m .config <required>
///   scripts/kconfig/merge_config.sh -m .config <hardening>
///   scripts/kconfig/merge_config.sh -m .config <extra>      (when Some)
///   make mod2yesconfig
///   make olddefconfig
///
/// `extra_fragment` is the optional caller-supplied `--kernel-config-fragment`.
/// When `Some`, it's merged after `hardening.config` so `mod2yesconfig` still
/// flattens any tristate symbols it introduces. When `None` the merge sequence
/// is byte-for-byte what it was before this fragment was added — kept this way
/// so the resolved config for callers that don't supply an extra fragment is
/// unchanged.
pub fn run_configure_phase(
    tools_tree: &Path,
    kernel_dir: &Path,
    required_fragment: &Path,
    hardening_fragment: &Path,
    extra_fragment: Option<&Path>,
) -> Result<()> {
    let kernel_dir_abs = kernel_dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", kernel_dir.display()))?;
    let required_abs = required_fragment.canonicalize()?;
    let hardening_abs = hardening_fragment.canonicalize()?;
    let extra_abs = extra_fragment.map(|p| p.canonicalize()).transpose()?;

    // Stage fragments inside the kernel dir so merge_config can find them
    // at relative paths under /build inside the nspawn.
    let frag_dir_in_kernel = kernel_dir_abs.join(".fragments");
    fs_err::create_dir_all(&frag_dir_in_kernel)?;
    fs_err::copy(&required_abs, frag_dir_in_kernel.join("required.config"))?;
    fs_err::copy(&hardening_abs, frag_dir_in_kernel.join("hardening.config"))?;
    if let Some(ref e) = extra_abs {
        fs_err::copy(e, frag_dir_in_kernel.join("extra.config"))?;
    }

    let extra_line = if extra_abs.is_some() {
        "scripts/kconfig/merge_config.sh -m .config .fragments/extra.config\n"
    } else {
        ""
    };
    // After olddefconfig, verify every `CONFIG_X=y` the EXTRA (caller) fragment
    // requested actually survived into .config. olddefconfig silently drops any
    // option with an unmet Kconfig dependency — no error, the symbol just isn't
    // in the built kernel (e.g. NETFILTER_XT_MATCH_OWNER dropped because
    // NETFILTER_ADVANCED was unset, surfacing only as a runtime failure much
    // later). Catch it here at config time — before the long rootfs build — so a
    // dropped option fails the build with the exact symbol named, not a mystery
    // downstream. Scoped to the caller's fragment: required/hardening are
    // steep's own, already consistent; the caller's is where unmet-dep mistakes
    // land. (`=m` collapses to `=y` via mod2yesconfig, so checking `=y` is
    // sufficient for this kernel's module-less build.)
    let verify_extra = if extra_abs.is_some() {
        "echo '=== verifying requested extra-fragment options survived ==='\n\
         missing=\"\"\n\
         while IFS= read -r line; do\n\
           case \"$line\" in CONFIG_*=y) ;; *) continue ;; esac\n\
           sym=\"${line%%=*}\"\n\
           grep -qxF \"${sym}=y\" .config || missing=\"$missing $sym\"\n\
         done < .fragments/extra.config\n\
         if [ -n \"$missing\" ]; then\n\
           echo \"FATAL: kernel options requested in the config fragment were dropped by\" >&2\n\
           echo \"olddefconfig (unmet Kconfig dependency or removed symbol):\" >&2\n\
           for s in $missing; do echo \"  - $s\" >&2; done\n\
           echo \"Add the missing dependency to the fragment and rebuild.\" >&2\n\
           exit 1\n\
         fi\n"
    } else {
        ""
    };
    let script = format!(
        "set -eux\n\
         cd /build\n\
         make x86_64_defconfig\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/required.config\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/hardening.config\n\
         {extra_line}\
         make mod2yesconfig\n\
         make olddefconfig\n\
         {verify_extra}",
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
    fn update_snapshot_overwrites_and_reports_change() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=n\n");
        let changed = update_snapshot(&a, &b).unwrap();
        assert!(changed);
        assert_eq!(fs_err::read_to_string(&b).unwrap(), "CONFIG_X=y\n");
    }

    #[test]
    fn update_snapshot_reports_unchanged_when_identical() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=y\n");
        let changed = update_snapshot(&a, &b).unwrap();
        assert!(!changed);
    }

    #[test]
    fn update_snapshot_creates_missing_snapshot_and_reports_change() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = d.path().join("new-snapshot");
        let changed = update_snapshot(&a, &b).unwrap();
        assert!(changed);
        assert_eq!(fs_err::read_to_string(&b).unwrap(), "CONFIG_X=y\n");
    }
}
