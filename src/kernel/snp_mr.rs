//! Kernel-held SEV-SNP runtime measurement registers.
//!
//! The version-specific patch adds the registers, their event log and the
//! REPORT_DATA commitment to the sev-guest driver. It is a cache input.

use std::path::Path;

use anyhow::Result;

use super::config;

pub const PATCH: &str = "kernel/patches/0002-sev-guest-runtime-measurement-registers.patch";
const STAGED_PATCH: &str = "confos-snp-mr.patch";
const STAGE_SCRIPT: &str = "set -eu\n\
    cd /build\n\
    patch --batch --forward --fuzz=0 -p1 < confos-snp-mr.patch\n";

/// Copy the patch into the freshly extracted kernel tree and apply it inside
/// the pinned tools tree, before Kconfig sees the tree.
pub fn stage(tools_tree: &Path, kernel_src: &Path) -> Result<()> {
    fs_err::copy(PATCH, kernel_src.join(STAGED_PATCH))?;
    config::nspawn(tools_tree, kernel_src, "/build", &[], STAGE_SCRIPT)
}
