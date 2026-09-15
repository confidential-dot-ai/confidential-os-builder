//! Mandatory trusted-AML kernel inputs and resolved-config checks.
//!
//! The source table and the single, version-specific kernel patch are both
//! cache inputs. The kernel enforces provenance before AML namespace parsing.

use std::path::Path;

use anyhow::{bail, Result};

use super::config;

pub const DSDT_SOURCE: &str = "kernel/trusted-dsdt.asl";
pub const PATCH: &str = "kernel/patches/0001-acpi-trusted-aml.patch";
pub const HEADER: &str = "confos-trusted-dsdt.h";
const STAGED_DSDT: &str = "confos-trusted-dsdt.asl";
const STAGED_PATCH: &str = "confos-trusted-aml.patch";
/// Trailer in the patch header naming the kernel its hooks were audited on.
const VERSION_TRAILER: &str = "Linux-Version:";
/// Applies the patch and compiles the table, run from the tree root inside
/// the pinned tools tree. Every name is a fixed builder-owned string; no
/// caller text reaches the shell.
const STAGE_SCRIPT: &str = "set -eu\n\
    cd /build\n\
    patch --batch --forward --fuzz=0 -p1 < confos-trusted-aml.patch\n\
    iasl -ve -tc -p dsdt confos-trusted-dsdt.asl\n\
    grep -q 'unsigned char dsdt_aml_code\\[\\]' dsdt.hex\n\
    cp dsdt.hex include/confos-trusted-dsdt.h\n";

/// Resolved `.config` lines that must hold once every fragment has merged.
/// The header path is checked as well as the booleans: selecting another
/// valid built-in DSDT would otherwise satisfy Kconfig while changing our
/// contract.
const REQUIRED: [(&str, &str); 4] = [
    ("CONFIG_ACPI", "y"),
    ("CONFIG_ACPI_CUSTOM_DSDT", "y"),
    ("CONFIG_ACPI_TRUSTED_AML", "y"),
    ("CONFIG_ACPI_CUSTOM_DSDT_FILE", "\"confos-trusted-dsdt.h\""),
];
/// Alternate table loaders that must stay off (`=y` and `=m` both fail).
const FORBIDDEN: [&str; 4] = [
    "CONFIG_ACPI_TABLE_UPGRADE",
    "CONFIG_ACPI_CONFIGFS",
    "CONFIG_EFI_CUSTOM_SSDT_OVERLAYS",
    "CONFIG_ACPI_DEBUGGER",
];

/// The patch declares the one kernel it was audited against; `kernel/version`
/// must match it. A pin bump therefore fails until the loader hooks are
/// re-audited and the patch retargeted, and each pin has exactly one home.
pub fn verify_version(version: &str) -> Result<()> {
    let patch = fs_err::read_to_string(PATCH)?;
    let supported = supported_version(&patch)?;
    if version != supported {
        bail!(
            "{PATCH} supports Linux {supported}, got {version}; re-audit the ACPI loader \
             hooks, retarget the patch and update its {VERSION_TRAILER} trailer"
        );
    }
    Ok(())
}

fn supported_version(patch: &str) -> Result<&str> {
    // Only the header carries the trailer; the diff body could quote it.
    let header = patch
        .split_once("\n---\n")
        .map_or(patch, |(header, _)| header);
    let mut versions = header
        .lines()
        .filter_map(|line| line.strip_prefix(VERSION_TRAILER))
        .map(str::trim)
        .filter(|v| !v.is_empty());
    match (versions.next(), versions.next()) {
        (Some(v), None) => Ok(v),
        (None, _) => bail!("{PATCH} has no {VERSION_TRAILER} trailer in its header"),
        (Some(_), Some(_)) => bail!("{PATCH} has more than one {VERSION_TRAILER} trailer"),
    }
}

/// Copy the ASL source and the patch into the freshly extracted kernel tree,
/// then apply the patch and compile the table inside the pinned tools tree.
/// Runs before Kconfig sees the tree, so the patch's new symbol resolves.
pub fn stage(tools_tree: &Path, kernel_src: &Path) -> Result<()> {
    fs_err::copy(DSDT_SOURCE, kernel_src.join(STAGED_DSDT))?;
    fs_err::copy(PATCH, kernel_src.join(STAGED_PATCH))?;
    config::nspawn(tools_tree, kernel_src, "/build", &[], STAGE_SCRIPT)
}

/// These invariants apply after all consumer fragments have been merged.
pub fn verify_config(config: &str) -> Result<()> {
    let values = config::values(config);
    for (symbol, value) in REQUIRED {
        if values.get(symbol) != Some(&value) {
            bail!("trusted AML requires resolved {symbol}={value}; consumer fragments cannot disable this boundary");
        }
    }
    for symbol in FORBIDDEN {
        if matches!(values.get(symbol), Some(&"y" | &"m")) {
            bail!("trusted AML forbids {symbol}; alternate table loaders must remain disabled");
        }
    }
    Ok(())
}

/// A resolved `.config` excerpt satisfying [`verify_config`]; shared with
/// the config tests so a new requirement is added in one place.
#[cfg(test)]
pub(crate) const VALID_CONFIG: &str = "CONFIG_ACPI=y\nCONFIG_ACPI_CUSTOM_DSDT=y\nCONFIG_ACPI_TRUSTED_AML=y\nCONFIG_ACPI_CUSTOM_DSDT_FILE=\"confos-trusted-dsdt.h\"\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_script_names_the_staged_inputs_and_header() {
        assert!(STAGE_SCRIPT.contains(&format!("-p1 < {STAGED_PATCH}\n")));
        assert!(STAGE_SCRIPT.contains(&format!("-p dsdt {STAGED_DSDT}\n")));
        assert!(STAGE_SCRIPT.ends_with(&format!("include/{HEADER}\n")));
    }

    #[test]
    fn rejects_weakened_consumer_configuration() {
        verify_config(VALID_CONFIG).unwrap();
        for required in VALID_CONFIG.lines() {
            let weakened = VALID_CONFIG.replace(required, "");
            assert!(
                verify_config(&weakened).is_err(),
                "accepted missing {required}"
            );
        }
        assert!(verify_config(&VALID_CONFIG.replace(HEADER, "other-dsdt.h")).is_err());
        for loader in FORBIDDEN {
            for value in ["y", "m"] {
                assert!(verify_config(&format!("{VALID_CONFIG}{loader}={value}\n")).is_err());
            }
        }
        // A disabled loader is fine; only enabled ones are forbidden.
        verify_config(&format!(
            "{VALID_CONFIG}# CONFIG_ACPI_TABLE_UPGRADE is not set\n"
        ))
        .unwrap();
    }

    #[test]
    fn supported_version_comes_from_the_header_trailer_only() {
        let patch =
            "Subject: x\n\nLinux-Version: 6.18.49\n---\n--- a/f\n+++ b/f\n+Linux-Version: 9.9.9\n";
        assert_eq!(supported_version(patch).unwrap(), "6.18.49");
        assert!(supported_version("Subject: x\n---\n").is_err());
        assert!(supported_version("Linux-Version: 1\nLinux-Version: 2\n---\n").is_err());
        assert!(supported_version("Linux-Version:   \n---\n").is_err());
    }

    #[test]
    fn committed_patch_names_the_pinned_kernel() {
        let version =
            crate::kernel::version::KernelVersion::read(Path::new("kernel/version")).unwrap();
        verify_version(&version.linux_version).unwrap();
        let pinned = version.linux_version;
        for other in [
            "6.16.12",
            &format!("{pinned}0"),
            &format!("{pinned}-extra"),
            "",
        ] {
            assert!(verify_version(other).is_err(), "accepted {other:?}");
        }
    }
}
