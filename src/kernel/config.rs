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
/// then verifies the merged fragments' boolean requests (after
/// last-fragment-wins overrides) against the resolved `.config`, failing
/// the build if `olddefconfig` silently dropped an on-request or
/// force-enabled an off-request (see [`verify_fragment_options`]).
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

/// Fail if the resolved `.config` disagrees with what the fragments
/// requested, in either direction. `olddefconfig` silently drops a
/// `CONFIG_X=y` whose Kconfig dependency is unmet (e.g.
/// NETFILTER_XT_MATCH_OWNER dropped because NETFILTER_ADVANCED was unset),
/// and just as silently force-enables a `# CONFIG_X is not set` when the
/// symbol is promptless or select'ed by an enabled one (e.g. MOUSE_PS2
/// selects SERIO_I8042) — both otherwise surface only as runtime
/// misbehavior or a quietly weaker kernel, long after the expensive build.
/// (`=m` counts as an on-request: mod2yesconfig runs after all fragments
/// merge and collapses it to `=y`, even when a profile fragment enables
/// CONFIG_MODULES. Non-boolean values are not verified.)
///
/// Fragments merge with last-wins semantics (see [`run_configure_phase`]),
/// so only each symbol's final requested state is checked: a later fragment
/// that sets `# CONFIG_X is not set` retracts an earlier `CONFIG_X=y`
/// request (e.g. c8s.config disables hardening.config's RANDSTRUCT_FULL,
/// which DEBUG_INFO_BTF is incompatible with) — and vice versa.
///
/// A fragment may also assert `# CONFIG_X is forced on` for a symbol it
/// wants off but that the enabled stack select's anyway. The marker must be
/// paired with a real off request, so the symbol must resolve `=y` despite
/// that request and the assertion fails loudly when the forcing goes away.
/// (An actual `=y` pin would keep the symbol on silently, so combining a pin
/// with an assertion is rejected.)
fn verify_fragment_options(fragments: &[&Path], resolved: &Path) -> Result<()> {
    /// Final request for a symbol after last-fragment-wins merging; On/Off
    /// carry the requesting fragment's name for the error message.
    enum Request {
        /// `=y` (or `=m`: mod2yesconfig collapses it); must resolve `=y`.
        On(String),
        /// `# is not set` or `=n`; must resolve off (not-set comment or
        /// absent).
        Off(String),
        /// `# CONFIG_X is forced on`: a symbol wanted off that the enabled
        /// stack select's anyway. Must have an effective off request and
        /// resolve `=y`, so the build fails if the forcing goes away.
        Forced(String),
    }
    /// Last merge-visible request for a symbol. Forced markers are metadata
    /// comments and deliberately do not update this state.
    enum SubmittedRequest {
        On,
        Off,
        Scalar,
    }
    fn valid_symbol(symbol: &str) -> bool {
        symbol.strip_prefix("CONFIG_").is_some_and(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
    }

    let config = fs_err::read_to_string(resolved)?;
    // symbol -> value from the resolved .config's `CONFIG_X=value` lines.
    let resolved_values: std::collections::HashMap<&str, &str> = config
        .lines()
        .filter(|l| l.starts_with("CONFIG_"))
        .filter_map(|l| l.split_once('='))
        .collect();
    let mut requested: std::collections::BTreeMap<String, Request> = Default::default();
    let mut submitted: std::collections::BTreeMap<String, SubmittedRequest> = Default::default();
    // Forced assertions are comments and therefore cannot retract an actual
    // on-request. Remember every on-pin independently of last-wins state so a
    // pin cannot make a forced assertion pass after its forcing chain vanishes.
    let mut on_pins: std::collections::BTreeMap<String, (String, String)> = Default::default();
    let mut forced: std::collections::BTreeMap<String, String> = Default::default();
    for frag in fragments {
        let name = frag.file_name().unwrap_or_default().to_string_lossy();
        for line in fs_err::read_to_string(frag)?.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("CONFIG_") {
                if line != trimmed {
                    return Err(anyhow!(
                        "{name}: ineffective request {line:?}; config assignments must \
                         start in column 0 and have no surrounding whitespace"
                    ));
                }
                let (symbol, value) = line.split_once('=').ok_or_else(|| {
                    anyhow!(
                        "{name}: ineffective request {line:?}; expected \
                         `CONFIG_SYMBOL=value`"
                    )
                })?;
                if !valid_symbol(symbol) {
                    return Err(anyhow!(
                        "{name}: ineffective request {line:?}; config symbols must match \
                         `CONFIG_[A-Za-z0-9_]+`"
                    ));
                }
                match value {
                    "y" | "m" => {
                        submitted.insert(symbol.to_string(), SubmittedRequest::On);
                        on_pins
                            .entry(symbol.to_string())
                            .or_insert_with(|| (name.to_string(), value.to_string()));
                        requested.insert(symbol.to_string(), Request::On(name.to_string()));
                    }
                    // kconfig treats `=n` exactly like `# is not set`.
                    "n" => {
                        submitted.insert(symbol.to_string(), SubmittedRequest::Off);
                        requested.insert(symbol.to_string(), Request::Off(name.to_string()));
                    }
                    value
                        if value.trim_start().bytes().next().is_some_and(|b| {
                            matches!(b.to_ascii_lowercase(), b'y' | b'm' | b'n')
                        }) =>
                    {
                        // Kconfig accepts a lowercase boolean by its first
                        // character, while other boolean-like spellings are
                        // ineffective. Require the complete canonical value so
                        // neither case silently escapes verification.
                        return Err(anyhow!(
                            "{name}: non-canonical boolean value in {line:?}; use exactly \
                             `{symbol}=y`, `{symbol}=m`, or `{symbol}=n`"
                        ));
                    }
                    // Scalar/string request: not verified and, crucially,
                    // does not erase an earlier boolean request.
                    _ => {
                        submitted.insert(symbol.to_string(), SubmittedRequest::Scalar);
                    }
                }
                continue;
            }

            // Classify near-miss markers after trimming and whitespace
            // normalization, then require their exact kconfig spelling. This
            // catches leading whitespace, `#CONFIG_X`, tabs/double spaces,
            // trailing text, and invalid symbol characters instead of
            // silently treating them as comments.
            let Some(comment) = trimmed.strip_prefix('#') else {
                continue;
            };
            let words: Vec<&str> = comment.split_whitespace().collect();
            let (symbol, is_forced) = match words.as_slice() {
                [symbol, "is", "not", "set", ..] if symbol.starts_with("CONFIG_") => {
                    (*symbol, false)
                }
                [symbol, "is", "forced", "on", ..] if symbol.starts_with("CONFIG_") => {
                    (*symbol, true)
                }
                _ => continue,
            };
            if !valid_symbol(symbol) {
                return Err(anyhow!(
                    "{name}: ineffective request {line:?}; config symbols must match \
                     `CONFIG_[A-Za-z0-9_]+`"
                ));
            }
            let canonical = if is_forced {
                format!("# {symbol} is forced on")
            } else {
                format!("# {symbol} is not set")
            };
            if line != canonical {
                return Err(anyhow!(
                    "{name}: ineffective request {line:?}; use the exact spelling \
                     {canonical:?}"
                ));
            }
            if is_forced {
                forced
                    .entry(symbol.to_string())
                    .or_insert_with(|| name.to_string());
            } else {
                submitted.insert(symbol.to_string(), SubmittedRequest::Off);
                requested.insert(symbol.to_string(), Request::Off(name.to_string()));
            }
        }
    }
    let pin_conflicts: Vec<String> = forced
        .iter()
        .filter_map(|(symbol, forced_name)| {
            on_pins.get(symbol).map(|(pin_name, value)| {
                format!(
                    "  - {forced_name}: {symbol} asserted forced on, but {pin_name} pins \
                     {symbol}={value}"
                )
            })
        })
        .collect();
    if !pin_conflicts.is_empty() {
        return Err(anyhow!(
            "forced-on assertions must not have an =y/=m pin in any fragment; \
             the pin would keep the symbol on and hide a vanished forcing chain:\n{}",
            pin_conflicts.join("\n")
        ));
    }

    let missing_off_requests: Vec<String> = forced
        .iter()
        .filter(|(symbol, _)| !matches!(submitted.get(*symbol), Some(SubmittedRequest::Off)))
        .map(|(symbol, name)| {
            format!("  - {name}: {symbol} asserted forced on without an effective off request")
        })
        .collect();
    if !missing_off_requests.is_empty() {
        return Err(anyhow!(
            "forced-on assertions must be paired with a final `# CONFIG_X is not set` \
             or `CONFIG_X=n` request; otherwise a default-y symbol can satisfy the \
             assertion after its forcing chain disappears:\n{}",
            missing_off_requests.join("\n")
        ));
    }

    // Assertion metadata is orthogonal to fragment merge order. Once its
    // effective off request is proven above, evaluate the symbol as Forced
    // even when the marker appeared before that request.
    for (symbol, name) in &forced {
        requested.insert(symbol.clone(), Request::Forced(name.clone()));
    }

    let mut mismatches = Vec::new();
    for (symbol, request) in &requested {
        match (request, resolved_values.get(symbol.as_str())) {
            (Request::On(_), Some(&"y")) => {}
            // Absent also covers a typo'd or removed symbol, which passes
            // silently; dependency-hidden symbols are absent too, so
            // treating absence as an error would false-positive.
            (Request::Off(_), None) => {}
            (Request::On(name), Some(v)) => {
                mismatches.push(format!(
                    "  - {name}: {symbol} requested on, got {symbol}={v}"
                ));
            }
            (Request::On(name), None) => {
                mismatches.push(format!("  - {name}: {symbol} requested on, dropped"));
            }
            (Request::Off(name), Some(v)) => {
                mismatches.push(format!(
                    "  - {name}: {symbol} requested off, got {symbol}={v}"
                ));
            }
            (Request::Forced(_), Some(&"y")) => {}
            (Request::Forced(name), Some(v)) => {
                mismatches.push(format!(
                    "  - {name}: {symbol} asserted forced on, got {symbol}={v}"
                ));
            }
            (Request::Forced(name), None) => {
                mismatches.push(format!(
                    "  - {name}: {symbol} asserted forced on, resolved off (forcing gone; \
                     drop the assertion and keep the off request)"
                ));
            }
        }
    }
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "kernel options requested in a config fragment do not match the resolved \
             .config (olddefconfig drops =y requests with unmet dependencies and \
             force-enables `is not set` requests for selected/promptless symbols):\n{}\n\
             Fix the fragment (add the missing dependency, or unset the selecting \
             symbol / pair its off request with `# CONFIG_X is forced on`) and rebuild.",
            mismatches.join("\n")
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
        let resolved = write(&d, "resolved", "CONFIG_A=y\nCONFIG_B=y\nCONFIG_C=y\n");
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
    fn verify_fragment_options_ignores_comment_lines() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# CONFIG_B=y\n# plain comment\n");
        let resolved = write(&d, "resolved", "CONFIG_C=y\n");
        verify_fragment_options(&[frag.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_passes_when_off_request_resolves_off() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# CONFIG_A is not set\n");
        // Off in the resolved config as absent (A) or as a not-set comment.
        let frag2 = write(&d, "frag2.config", "# CONFIG_B is not set\n");
        let resolved = write(&d, "resolved", "# CONFIG_B is not set\nCONFIG_C=y\n");
        verify_fragment_options(&[frag.as_path(), frag2.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_fails_when_off_request_is_force_enabled() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "hardening.config", "# CONFIG_SERIO_I8042 is not set\n");
        let resolved = write(&d, "resolved", "CONFIG_SERIO_I8042=y\n");
        let err = verify_fragment_options(&[frag.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("hardening.config: CONFIG_SERIO_I8042 requested off"));
    }

    #[test]
    fn verify_fragment_options_reports_module_value_for_failed_off_request() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# CONFIG_A is not set\n");
        let resolved = write(&d, "resolved", "CONFIG_A=m\n");
        let err = verify_fragment_options(&[frag.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("frag.config: CONFIG_A requested off, got CONFIG_A=m"));
    }

    #[test]
    fn verify_fragment_options_rejects_not_set_line_with_trailing_text() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "# CONFIG_A is not set  # rationale\n");
        let resolved = write(&d, "resolved", "CONFIG_B=y\n");
        let err = verify_fragment_options(&[frag.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ineffective request"));
    }

    #[test]
    fn verify_fragment_options_treats_eq_n_as_off_request() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "CONFIG_A=n\n");
        let forced = write(&d, "resolved", "CONFIG_A=y\n");
        let err = verify_fragment_options(&[frag.as_path()], &forced)
            .unwrap_err()
            .to_string();
        assert!(err.contains("frag.config: CONFIG_A requested off"));
        let off = write(&d, "resolved2", "CONFIG_B=y\n");
        verify_fragment_options(&[frag.as_path()], &off).unwrap();
    }

    #[test]
    fn verify_fragment_options_treats_module_request_as_on_request() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "CONFIG_A=m\n");
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");
        verify_fragment_options(&[frag.as_path()], &resolved).unwrap();
        let dropped = write(&d, "resolved2", "CONFIG_B=y\n");
        assert!(verify_fragment_options(&[frag.as_path()], &dropped).is_err());
    }

    #[test]
    fn verify_fragment_options_reports_resolved_value_on_clamped_on_request() {
        let d = TempDir::new().unwrap();
        let frag = write(&d, "frag.config", "CONFIG_A=y\n");
        let resolved = write(&d, "resolved", "CONFIG_A=m\n");
        let err = verify_fragment_options(&[frag.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("frag.config: CONFIG_A requested on, got CONFIG_A=m"));
    }

    #[test]
    fn verify_fragment_options_checks_forced_assertions() {
        let d = TempDir::new().unwrap();
        let held = write(&d, "resolved", "CONFIG_A=y\n");
        let gone = write(&d, "resolved2", "CONFIG_B=y\n");
        let clamped = write(&d, "resolved3", "CONFIG_A=m\n");

        for content in [
            "# CONFIG_A is not set\n# CONFIG_A is forced on\n",
            "# CONFIG_A is forced on\n# CONFIG_A is not set\n",
        ] {
            let frag = write(&d, "frag.config", content);
            verify_fragment_options(&[frag.as_path()], &held).unwrap();
            let err = verify_fragment_options(&[frag.as_path()], &gone)
                .unwrap_err()
                .to_string();
            assert!(err.contains("frag.config: CONFIG_A asserted forced on, resolved off"));
            let err = verify_fragment_options(&[frag.as_path()], &clamped)
                .unwrap_err()
                .to_string();
            assert!(err.contains("frag.config: CONFIG_A asserted forced on, got CONFIG_A=m"));
        }
    }

    #[test]
    fn verify_fragment_options_rejects_forced_assertion_without_effective_off_request() {
        let d = TempDir::new().unwrap();
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");
        for content in [
            "# CONFIG_A is forced on\n",
            "# CONFIG_A is not set\nCONFIG_A=42\n# CONFIG_A is forced on\n",
        ] {
            let frag = write(&d, "frag.config", content);
            let err = verify_fragment_options(&[frag.as_path()], &resolved)
                .unwrap_err()
                .to_string();
            assert!(err.contains("without an effective off request"));
        }
    }

    #[test]
    fn verify_fragment_options_rejects_forced_assertion_with_pin_in_either_order() {
        let d = TempDir::new().unwrap();
        let pin = write(&d, "pin.config", "CONFIG_A=y\n");
        let assertion = write(
            &d,
            "assert.config",
            "# CONFIG_A is not set\n# CONFIG_A is forced on\n",
        );
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");

        for fragments in [
            [pin.as_path(), assertion.as_path()],
            [assertion.as_path(), pin.as_path()],
        ] {
            let err = verify_fragment_options(&fragments, &resolved)
                .unwrap_err()
                .to_string();
            assert!(err.contains("CONFIG_A asserted forced on"));
            assert!(err.contains("pin.config pins CONFIG_A=y"));
        }
    }

    #[test]
    fn verify_fragment_options_rejects_noncanonical_boolean_values() {
        let d = TempDir::new().unwrap();
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");
        for value in ["yes", "modules", "no", "Y"] {
            let frag = write(&d, "frag.config", &format!("CONFIG_A={value}\n"));
            let err = verify_fragment_options(&[frag.as_path()], &resolved)
                .unwrap_err()
                .to_string();
            assert!(err.contains("non-canonical boolean value"));
            assert!(err.contains(&format!("CONFIG_A={value}")));
        }
    }

    #[test]
    fn verify_fragment_options_rejects_ineffective_request_spellings() {
        let d = TempDir::new().unwrap();
        let resolved = write(&d, "resolved", "CONFIG_A=y\n");
        for line in [
            "  CONFIG_A=y",
            "CONFIG_A = y",
            "CONFIG_A-B=y",
            "#CONFIG_A is not set",
            "#  CONFIG_A is not set",
            "#\tCONFIG_A is not set",
            " # CONFIG_A is not set",
            "# CONFIG_A  is not set",
            "# CONFIG_A is forced on  # rationale",
        ] {
            let frag = write(&d, "frag.config", &format!("{line}\n"));
            let err = verify_fragment_options(&[frag.as_path()], &resolved)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("ineffective request"),
                "unexpected error for {line:?}: {err}"
            );
        }
    }

    #[test]
    fn verify_fragment_options_skips_non_boolean_values() {
        let d = TempDir::new().unwrap();
        let frag = write(
            &d,
            "frag.config",
            "CONFIG_PANIC_TIMEOUT=-1\nCONFIG_PATH=\"\"\n",
        );
        let resolved = write(&d, "resolved", "CONFIG_C=y\n");
        verify_fragment_options(&[frag.as_path()], &resolved).unwrap();
    }

    #[test]
    fn verify_fragment_options_scalar_does_not_erase_earlier_boolean_request() {
        let d = TempDir::new().unwrap();
        let first = write(&d, "first.config", "CONFIG_A=y\n");
        let scalar = write(&d, "scalar.config", "CONFIG_A=42\n");
        let resolved = write(&d, "resolved", "CONFIG_B=y\n");
        let err = verify_fragment_options(&[first.as_path(), scalar.as_path()], &resolved)
            .unwrap_err()
            .to_string();
        assert!(err.contains("first.config: CONFIG_A requested on, dropped"));
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
