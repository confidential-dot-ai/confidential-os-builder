//! Mandatory trusted-AML kernel inputs and resolved-config checks.
//!
//! The source table and the single, version-specific kernel patch are both
//! cache inputs. The kernel enforces provenance before AML namespace parsing.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Result};

pub const DSDT_SOURCE: &str = "kernel/trusted-dsdt.asl";
pub const PATCH: &str = "kernel/patches/0001-acpi-trusted-aml.patch";
pub const HEADER: &str = "confos-trusted-dsdt.h";
const STAGED_DSDT: &str = "confos-trusted-dsdt.asl";
const STAGED_PATCH: &str = "confos-trusted-aml.patch";
/// Trailer in the patch header naming the kernel its hooks were audited on.
const VERSION_TRAILER: &str = "Linux-Version:";

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
    let header = patch.split("\n---\n").next().unwrap_or(patch);
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

/// Copy the ASL source and the patch into the freshly extracted kernel tree
/// and return the script that applies the patch and compiles the table.
/// The caller runs it inside the pinned tools tree from the tree root,
/// before Kconfig sees the tree, so the patch's new symbol resolves. All
/// names are fixed builder-owned strings; no caller text reaches the shell.
pub fn stage(kernel_src: &Path) -> Result<String> {
    fs_err::copy(DSDT_SOURCE, kernel_src.join(STAGED_DSDT))?;
    fs_err::copy(PATCH, kernel_src.join(STAGED_PATCH))?;
    Ok(format!(
        "patch --batch --forward --fuzz=0 -p1 < {STAGED_PATCH}\n\
         iasl -ve -tc -p dsdt {STAGED_DSDT}\n\
         grep -q 'unsigned char dsdt_aml_code\\[\\]' dsdt.hex\n\
         cp dsdt.hex include/{HEADER}\n"
    ))
}

/// These invariants apply after all consumer fragments have been merged.
/// Check the custom-header path as well as booleans: selecting another valid
/// built-in DSDT would otherwise satisfy Kconfig while changing our contract.
pub fn verify_config(config: &str) -> Result<()> {
    let lines: HashSet<&str> = config.lines().map(str::trim).collect();
    let header = format!("CONFIG_ACPI_CUSTOM_DSDT_FILE=\"{HEADER}\"");
    for required in [
        "CONFIG_ACPI=y",
        "CONFIG_ACPI_CUSTOM_DSDT=y",
        "CONFIG_ACPI_TRUSTED_AML=y",
        header.as_str(),
    ] {
        if !lines.contains(required) {
            bail!("trusted AML requires resolved {required}; consumer fragments cannot disable this boundary");
        }
    }
    for forbidden in [
        "CONFIG_ACPI_TABLE_UPGRADE",
        "CONFIG_ACPI_CONFIGFS",
        "CONFIG_EFI_CUSTOM_SSDT_OVERLAYS",
        "CONFIG_ACPI_DEBUGGER",
    ] {
        if ["y", "m"]
            .iter()
            .any(|v| lines.contains(format!("{forbidden}={v}").as_str()))
        {
            bail!("trusted AML forbids {forbidden}; alternate table loaders must remain disabled");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = "CONFIG_ACPI=y\nCONFIG_ACPI_CUSTOM_DSDT=y\nCONFIG_ACPI_TRUSTED_AML=y\nCONFIG_ACPI_CUSTOM_DSDT_FILE=\"confos-trusted-dsdt.h\"\n";

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
        for loader in [
            "ACPI_TABLE_UPGRADE",
            "ACPI_CONFIGFS",
            "EFI_CUSTOM_SSDT_OVERLAYS",
            "ACPI_DEBUGGER",
        ] {
            for value in ["y", "m"] {
                assert!(
                    verify_config(&format!("{VALID_CONFIG}CONFIG_{loader}={value}\n")).is_err()
                );
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
