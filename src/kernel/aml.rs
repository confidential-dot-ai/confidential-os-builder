//! Mandatory trusted-AML kernel inputs and resolved-config checks.
//!
//! The source table and the single, version-specific kernel patch are both
//! cache inputs. The kernel enforces provenance before AML namespace parsing.

use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{config, fetch};

pub const DSDT_SOURCE: &str = "kernel/trusted-dsdt.asl";
pub const PATCH: &str = "kernel/patches/0001-acpi-trusted-aml.patch";
pub const HEADER: &str = "confos-trusted-dsdt.h";
const LINUX_VERSION: &str = "6.18.49";

/// A kernel update must re-audit the loader hooks and explicitly retarget the
/// patch. Do not let a cache hit bypass this compatibility gate.
pub fn verify_version(version: &str) -> Result<()> {
    if version != LINUX_VERSION {
        bail!("trusted AML patch supports Linux {LINUX_VERSION}, got {version}; re-audit the ACPI loader before updating the kernel");
    }
    Ok(())
}

/// Hashes of the actual files staged for this build, captured before compilation.
pub struct PreparedInputs {
    dsdt_sha256: String,
    patch_sha256: String,
}

impl PreparedInputs {
    fn read(dsdt: &Path, patch: &Path) -> Result<Self> {
        Ok(Self {
            dsdt_sha256: fetch::sha256_file(dsdt)?,
            patch_sha256: fetch::sha256_file(patch)?,
        })
    }

    /// Do not label an old build with newly edited repository inputs.
    pub fn verify(&self, dsdt_sha256: &str, patch_sha256: &str) -> Result<()> {
        if self.dsdt_sha256 != dsdt_sha256 || self.patch_sha256 != patch_sha256 {
            bail!("trusted AML inputs changed during compilation; rerun the kernel build");
        }
        Ok(())
    }
}

/// Apply the patch to freshly extracted, checksum-verified source and compile
/// ASL using the same pinned tools tree as the kernel. All shell paths below
/// are fixed builder-owned names; no caller text is interpolated into shell.
pub fn prepare(tools_tree: &Path, kernel_src: &Path) -> Result<PreparedInputs> {
    let dsdt = kernel_src.join("confos-trusted-dsdt.asl");
    let patch = kernel_src.join("confos-trusted-aml.patch");
    fs_err::copy(DSDT_SOURCE, &dsdt)?;
    fs_err::copy(PATCH, &patch)?;
    let staged = PreparedInputs::read(&dsdt, &patch)?;
    config::nspawn(
        tools_tree,
        &kernel_src.canonicalize()?,
        "/build",
        &[],
        "set -eu\n\
         cd /build\n\
         patch --batch --forward --fuzz=0 --dry-run -p1 < confos-trusted-aml.patch\n\
         patch --batch --forward --fuzz=0 -p1 < confos-trusted-aml.patch\n\
         iasl -ve -tc -p dsdt confos-trusted-dsdt.asl\n\
         test -s dsdt.hex\n\
         cp dsdt.hex include/confos-trusted-dsdt.h\n",
    )
    .context("preparing measured AML kernel inputs")?;
    Ok(staged)
}

/// These invariants apply after all consumer fragments have been merged.
/// Check the custom-header path as well as booleans: selecting another valid
/// built-in DSDT would otherwise satisfy Kconfig while changing our contract.
pub fn verify_config(config: &str) -> Result<()> {
    for required in [
        "CONFIG_ACPI=y",
        "CONFIG_ACPI_CUSTOM_DSDT=y",
        "CONFIG_ACPI_TRUSTED_AML=y",
    ] {
        if !config.lines().any(|line| line.trim() == required) {
            bail!("trusted AML requires resolved {required}; consumer fragments cannot disable this boundary");
        }
    }
    let header = format!("CONFIG_ACPI_CUSTOM_DSDT_FILE=\"{HEADER}\"");
    if !config.lines().any(|line| line.trim() == header) {
        bail!("trusted AML requires resolved {header}");
    }
    for forbidden in [
        "CONFIG_ACPI_TABLE_UPGRADE",
        "CONFIG_ACPI_CONFIGFS",
        "CONFIG_EFI_CUSTOM_SSDT_OVERLAYS",
        "CONFIG_ACPI_DEBUGGER",
        "CONFIG_ACPI_CUSTOM_METHOD",
    ] {
        if config.lines().any(|line| {
            line.trim()
                .strip_prefix(forbidden)
                .is_some_and(|value| matches!(value, "=y" | "=m"))
        }) {
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
            "ACPI_CUSTOM_METHOD",
        ] {
            for value in ["y", "m"] {
                assert!(
                    verify_config(&format!("{VALID_CONFIG}CONFIG_{loader}={value}\n")).is_err()
                );
            }
        }
    }

    #[test]
    fn rejects_inputs_edited_after_staging() {
        let dir = tempfile::tempdir().unwrap();
        let dsdt = dir.path().join("dsdt.asl");
        let patch = dir.path().join("kernel.patch");
        fs_err::write(&dsdt, "original table").unwrap();
        fs_err::write(&patch, "original patch").unwrap();
        let staged = PreparedInputs::read(&dsdt, &patch).unwrap();
        let original_dsdt = fetch::sha256_file(&dsdt).unwrap();
        let original_patch = fetch::sha256_file(&patch).unwrap();
        staged.verify(&original_dsdt, &original_patch).unwrap();
        fs_err::write(&dsdt, "edited table").unwrap();
        assert!(staged
            .verify(&fetch::sha256_file(&dsdt).unwrap(), &original_patch)
            .is_err());
        fs_err::write(&patch, "edited patch").unwrap();
        assert!(staged
            .verify(&original_dsdt, &fetch::sha256_file(&patch).unwrap())
            .is_err());
    }

    #[test]
    fn kernel_version_changes_require_explicit_patch_review() {
        verify_version("6.18.49").unwrap();
        for version in ["6.16.12", "6.18.50", "6.18.49-extra", ""] {
            assert!(verify_version(version).is_err());
        }
    }
}
