//! Cache-aware artifact accessor for the custom kernel build.
//!
//! The cache check lives in `commands::kernel::run`. This module is a thin
//! wrapper that calls the builder, reads the resulting manifest, and returns
//! a `KernelArtifact` shaped for use by `commands::build`.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::commands;
use crate::kernel::manifest as km;
use crate::{KernelArgs, KernelInputs};

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
/// from `confos build`.
pub fn ensure_kernel(force: bool, inputs: KernelInputs) -> Result<KernelArtifact> {
    commands::kernel::run(&KernelArgs {
        force,
        output: PathBuf::from(KERNEL_OUT_DIR),
        kernel_inputs: inputs,
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
