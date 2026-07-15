//! Kernel `.config` resolution and snapshot lockfile.
//!
//! The "configure phase" runs `make x86_64_defconfig`, applies fragments,
//! then `mod2yesconfig`, then `olddefconfig`. The resolved `.config` is
//! then written back to the committed snapshot via [`update_snapshot`],
//! which confos tracks in git like a lockfile.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::tools;

/// Overwrite `snapshot` with the freshly-resolved `.config`, returning
/// whether the content changed (`true` if the snapshot was absent or
/// differed). confos calls this on every kernel build, so the snapshot
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
///   scripts/kconfig/merge_config.sh -m .config <confidential>
///   scripts/kconfig/merge_config.sh -m .config <extra>      (when Some)
///   make mod2yesconfig
///   make olddefconfig
///
/// then verifies every `CONFIG_X=y` requested by the merged fragments (after
/// last-fragment-wins overrides) is present in the resolved `.config`,
/// failing the build if `olddefconfig` silently dropped any (unmet Kconfig
/// dependency or removed symbol).
///
/// Merge order is important: `confidential.config` deliberately re-enables
/// options the `hardening.config` fragment turned off (e.g.
/// `CONFIG_ACPI_TABLE_UPGRADE=y` overriding the `# is not set` line) — last
/// fragment wins under `merge_config.sh`, so the confidential fragment MUST
/// follow hardening.
///
/// `extra_fragment` is the optional caller-supplied `--kernel-config-fragment`.
/// When `Some`, it's merged after the confos-controlled fragments so
/// `mod2yesconfig` still flattens any tristate symbols it introduces.
pub fn run_configure_phase(
    tools_tree: &Path,
    kernel_dir: &Path,
    required_fragment: &Path,
    hardening_fragment: &Path,
    confidential_fragment: &Path,
    extra_fragment: Option<&Path>,
) -> Result<()> {
    let kernel_dir_abs = kernel_dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", kernel_dir.display()))?;
    let required_abs = required_fragment.canonicalize()?;
    let hardening_abs = hardening_fragment.canonicalize()?;
    let confidential_abs = confidential_fragment.canonicalize()?;
    let extra_abs = extra_fragment.map(|p| p.canonicalize()).transpose()?;

    // Stage fragments inside the kernel dir so merge_config can find them
    // at relative paths under /build inside the nspawn.
    let frag_dir_in_kernel = kernel_dir_abs.join(".fragments");
    fs_err::create_dir_all(&frag_dir_in_kernel)?;
    fs_err::copy(&required_abs, frag_dir_in_kernel.join("required.config"))?;
    fs_err::copy(&hardening_abs, frag_dir_in_kernel.join("hardening.config"))?;
    fs_err::copy(
        &confidential_abs,
        frag_dir_in_kernel.join("confidential.config"),
    )?;
    if let Some(ref e) = extra_abs {
        fs_err::copy(e, frag_dir_in_kernel.join("extra.config"))?;
    }

    let extra_line = if extra_abs.is_some() {
        "scripts/kconfig/merge_config.sh -m .config .fragments/extra.config\n"
    } else {
        ""
    };
    let script = format!(
        "set -eux\n\
         cd /build\n\
         make x86_64_defconfig\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/required.config\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/hardening.config\n\
         scripts/kconfig/merge_config.sh -m .config .fragments/confidential.config\n\
         {extra_line}\
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

    let mut fragments: Vec<&Path> = vec![&required_abs, &hardening_abs, &confidential_abs];
    if let Some(ref e) = extra_abs {
        fragments.push(e);
    }
    verify_fragment_options(&fragments, &kernel_dir_abs.join(".config"))
}

/// Fail if any `CONFIG_X=y` requested by the fragments is absent from the
/// resolved `.config`. `olddefconfig` drops an option whose Kconfig
/// dependency is unmet (or whose symbol no longer exists) without any error
/// — the miss otherwise surfaces only as runtime misbehavior, long after the
/// expensive build (e.g. NETFILTER_XT_MATCH_OWNER silently dropped because
/// NETFILTER_ADVANCED was unset). (`=m` collapses to `=y` via mod2yesconfig
/// before olddefconfig, so checking `=y` is sufficient for this module-less
/// build.)
///
/// Fragments merge with last-wins semantics (see [`run_configure_phase`]),
/// so only each symbol's final requested state is checked: a later fragment
/// that sets `# CONFIG_X is not set` retracts an earlier `CONFIG_X=y`
/// request (e.g. c8s.config disables hardening.config's RANDSTRUCT_FULL,
/// which DEBUG_INFO_BTF is incompatible with).
fn verify_fragment_options(fragments: &[&Path], resolved: &Path) -> Result<()> {
    let config = fs_err::read_to_string(resolved)?;
    let enabled: std::collections::HashSet<&str> = config.lines().collect();
    // symbol -> Some((requesting fragment, `CONFIG_X=y` line)) for a live
    // `=y` request, None once a later fragment sets any other value.
    let mut requested: std::collections::BTreeMap<String, Option<(String, String)>> =
        Default::default();
    for frag in fragments {
        let name = frag.file_name().unwrap_or_default().to_string_lossy();
        for line in fs_err::read_to_string(frag)?.lines() {
            // Match on the first whitespace-delimited token so a trailing
            // comment can't smuggle a request past verification.
            let token = line.split_whitespace().next().unwrap_or("");
            if let Some((symbol, _)) = token
                .split_once('=')
                .filter(|_| token.starts_with("CONFIG_"))
            {
                let request = token
                    .ends_with("=y")
                    .then(|| (name.to_string(), token.to_string()));
                requested.insert(symbol.to_string(), request);
            } else if let Some(symbol) = line
                .strip_prefix("# ")
                .and_then(|r| r.strip_suffix(" is not set"))
                .filter(|s| s.starts_with("CONFIG_"))
            {
                requested.insert(symbol.to_string(), None);
            }
        }
    }
    let missing: Vec<String> = requested
        .into_values()
        .flatten()
        .filter(|(_, line)| !enabled.contains(line.as_str()))
        .map(|(name, line)| format!("  - {}: {}", name, line.trim_end_matches("=y")))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "kernel options requested in a config fragment were dropped by olddefconfig \
             (unmet Kconfig dependency or removed symbol):\n{}\n\
             Add the missing dependency to the fragment and rebuild.",
            missing.join("\n")
        ))
    }
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
    fn verify_fragment_options_passes_when_all_requested_options_present() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# comment\nCONFIG_A=y\nCONFIG_B=m\n");
        let resolved = write(&d, "resolved", "CONFIG_A=y\nCONFIG_C=y\n");
        verify_fragment_options(&[frag.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_names_dropped_symbol_and_fragment() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "extra.config", "CONFIG_A=y\nCONFIG_B=y\n");
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");
        let err = verify_fragment_options(&[frag.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("extra.config: CONFIG_B"));
        assert!(!err.contains("CONFIG_A"));
    }

    #[test]
    fn verify_fragment_options_ignores_not_set_and_comment_lines() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# CONFIG_A is not set\n# CONFIG_B=y\n");
        let resolved = write(&d, "resolved", "CONFIG_C=y\n");
        verify_fragment_options(&[frag.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_honors_later_fragment_disabling_earlier_request() {
        let d = TempDir::new().unwrap();
        let hardening = write(&d, "hardening.config", "CONFIG_A=y\nCONFIG_B=y\n");
        let extra = write(&d, "extra.config", "# CONFIG_A is not set\n");
        let resolved = write(&d, "resolved", "CONFIG_B=y\n");
        verify_fragment_options(&[hardening.as_path(), extra.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_attributes_reenabled_symbol_to_last_fragment() {
        let d = TempDir::new().unwrap();
        let hardening = write(&d, "hardening.config", "# CONFIG_A is not set\n");
        let extra = write(&d, "extra.config", "CONFIG_A=y\n");
        let resolved = write(&d, "resolved", "CONFIG_B=y\n");
        let err = verify_fragment_options(&[hardening.as_path(), extra.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("extra.config: CONFIG_A"));
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
