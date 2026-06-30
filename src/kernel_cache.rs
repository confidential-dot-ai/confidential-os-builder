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

const KERNEL_OUT_DIR: &str = "output/kernel";

pub struct KernelArtifact {
    pub vmlinuz_path: PathBuf,
    pub linux_version: String,
    pub manifest: km::KernelManifest,
}

/// Ensure a current kernel artifact exists at output/kernel/.
/// Force=true bypasses the cache (rebuilds from scratch).
///
/// `fragment` is the caller-supplied `--kernel-config-fragment`, threaded
/// from `steep build`.
pub fn ensure_kernel(
    force: bool,
    fragment: Option<PathBuf>,
    kernel_builder_package: Vec<String>,
) -> Result<KernelArtifact> {
    require_inputs_exist(fragment.as_deref())?;

    commands::kernel::run(&KernelArgs {
        force,
        output: PathBuf::from(KERNEL_OUT_DIR),
        kernel_config_fragment: fragment,
        kernel_builder_package,
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

fn require_inputs_exist(fragment: Option<&Path>) -> Result<()> {
    for f in [
        "kernel/version",
        "kernel/required.config",
        "kernel/hardening.config",
        "kernel/confidential.config",
    ] {
        if !Path::new(f).exists() {
            return Err(anyhow!("required file missing: {}", f));
        }
    }
    if let Some(frag) = fragment {
        if !frag.exists() {
            return Err(anyhow!(
                "--kernel-config-fragment path not found: {}",
                frag.display()
            ));
        }
    }
    Ok(())
}
