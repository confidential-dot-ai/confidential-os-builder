//! Cache-aware artifact accessor for the custom kernel build.
//!
//! The cache check lives in `commands::kernel::run`. This module is a thin
//! wrapper that calls the builder, reads the resulting manifest, and returns
//! a `KernelArtifact` shaped for use by `commands::build`; `seal` relinks
//! that artifact with an image's root hash in its IPE policy.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::commands;
use crate::kernel::manifest as km;
use crate::{KernelArgs, KernelInputs};

const KERNEL_OUT_DIR: &str = "output/kernel";

pub struct KernelArtifact {
    pub vmlinuz_path: PathBuf,
    pub linux_version: String,
    pub manifest: km::KernelManifest,
    inputs: KernelInputs,
}

/// Ensure a current kernel artifact exists at output/kernel/.
/// Force=true bypasses the cache (rebuilds from scratch).
///
/// `fragment` is the caller-supplied `--kernel-config-fragment`, threaded
/// from `confos build`.
pub fn ensure_kernel(force: bool, inputs: KernelInputs) -> Result<KernelArtifact> {
    require_inputs_exist(&inputs)?;

    commands::kernel::run(&KernelArgs {
        force,
        output: PathBuf::from(KERNEL_OUT_DIR),
        kernel_inputs: inputs.clone(),
    })?;

    let manifest_path = Path::new(KERNEL_OUT_DIR).join("manifest.json");
    let vmlinuz_path = Path::new(KERNEL_OUT_DIR).join("vmlinuz");
    let manifest = km::read(&manifest_path)?;
    Ok(KernelArtifact {
        vmlinuz_path,
        linux_version: manifest.linux_version.clone(),
        manifest,
        inputs,
    })
}

/// Relink the cached kernel with `roothash` sealed into its IPE policy,
/// writing the sealed `vmlinuz` (plus the policy and log) into `out_dir`.
/// Re-reads the cache manifest afterwards: sealing rebuilds the base kernel
/// when its build tree is missing, and the artifact must describe that.
pub fn seal(kernel: &mut KernelArtifact, roothash: &str, out_dir: &Path) -> Result<PathBuf> {
    let cache_dir = Path::new(KERNEL_OUT_DIR);
    let sealed = commands::kernel::seal(cache_dir, &kernel.inputs, roothash, out_dir)?;
    kernel.manifest = km::read(&cache_dir.join("manifest.json"))?;
    Ok(sealed)
}

fn require_inputs_exist(inputs: &KernelInputs) -> Result<()> {
    let fragment = inputs.kernel_config_fragment.as_deref();
    if let Some(c) = inputs.module_signing_cert.as_deref() {
        if !c.is_file() {
            anyhow::bail!("--module-signing-cert path not found: {}", c.display());
        }
    }
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
