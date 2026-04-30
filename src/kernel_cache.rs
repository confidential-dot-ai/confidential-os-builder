//! Cache-aware artifact accessor for the custom kernel build.
//!
//! The cache check lives in `commands::kernel::run`. This module is a thin
//! wrapper that calls the builder, reads the resulting manifest, and returns
//! a `KernelArtifact` shaped for use by `commands::build`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::commands;
use crate::kernel::manifest as km;
use crate::KernelArgs;

const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const KERNEL_OUT_DIR: &str = "output/kernel";

pub struct KernelArtifact {
    pub vmlinuz_path: PathBuf,
    pub linux_version: String,
    pub manifest: km::KernelManifest,
}

/// Ensure a current kernel artifact exists at output/kernel/.
/// Force=true bypasses the cache (rebuilds from scratch).
pub fn ensure_kernel(force: bool) -> Result<KernelArtifact> {
    require_inputs_exist()?;

    commands::kernel::run(&KernelArgs {
        force,
        update_snapshot: false,
        output: PathBuf::from(KERNEL_OUT_DIR),
    })?;

    let manifest_path = Path::new(KERNEL_OUT_DIR).join("manifest.json");
    let vmlinuz_path = Path::new(KERNEL_OUT_DIR).join("vmlinuz");
    let manifest = km::read(&manifest_path)?;
    Ok(KernelArtifact {
        vmlinuz_path,
        linux_version: manifest.linux_version.clone(),
        manifest,
    })
}

fn require_inputs_exist() -> Result<()> {
    for f in [
        "kernel/version",
        "kernel/required.config",
        "kernel/hardening.config",
    ] {
        if !Path::new(f).exists() {
            return Err(anyhow!("required file missing: {}", f));
        }
    }
    if !Path::new(SNAPSHOT_PATH).exists() {
        return Err(anyhow!(
            "{} missing. Run `steep kernel --update-snapshot` to generate.",
            SNAPSHOT_PATH
        ));
    }
    Ok(())
}
