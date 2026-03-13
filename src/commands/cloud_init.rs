use std::path::Path;

use crate::mkosi::config::MkosiConfig;
use crate::igvm::invoke::IgvmBuildArgs;
use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
};
use crate::{tools, CloudInitArgs, ImageFormat};

fn validate_inputs(args: &CloudInitArgs) -> anyhow::Result<()> {
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    ensure_file_exists(&args.kernel, "kernel")?;
    ensure_file_exists(&args.initrd, "initrd")?;
    ensure_file_exists(&args.firmware, "firmware")?;
    ensure_file_exists(&args.base_image, "base image")?;
    Ok(())
}

fn ensure_file_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_file() {
        anyhow::bail!("{label} is not a file: {}", path.display());
    }
    Ok(())
}

fn ensure_dir_exists(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("{label} not found: {}", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("{label} is not a directory: {}", path.display());
    }
    Ok(())
}

fn format_extension(format: &ImageFormat) -> &'static str {
    match format {
        ImageFormat::Qcow2 => "qcow2",
        ImageFormat::Vhd => "vhd",
        ImageFormat::Raw => "raw",
    }
}

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Step 1: Validate inputs
    validate_inputs(args)?;

    // Step 2: Check required tools
    tools::require("mkosi")?;
    tools::require("ukify")?;
    tools::require("igvm-tools")?;
    tools::require("qemu-img")?;

    // Step 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Step 4: Build project partition via mkosi
    let work_dir = tempfile::tempdir()?;
    let mkosi_config = MkosiConfig::cloud_init(args.dir.clone());
    let mkosi_config_path = work_dir.path().join("mkosi.conf");
    mkosi_config.write_to(&mkosi_config_path)?;
    tracing::info!(config = %mkosi_config_path.display(), "generated mkosi config");

    // TODO: Invoke mkosi to build project partition
    let _project_partition = work_dir.path().join("project.raw");
    tracing::warn!("mkosi invocation not yet implemented — skipping project partition build");

    // Step 5: Compose disk image (base + project)
    let _raw_disk = args.output.join("disk.raw");
    tracing::warn!("disk composition not yet implemented — skipping");

    // Step 6: Build UKI via ukify
    let uki_path = args.output.join("uki.efi");
    tracing::warn!("UKI build not yet implemented — skipping");

    // Step 7: Build IGVM via igvm-tools
    let igvm_manifest_path = work_dir.path().join("igvm-manifest.json");
    let igvm_path = args.output.join("guest.igvm");
    let igvm_args = IgvmBuildArgs {
        firmware: args.firmware.clone(),
        kernel: uki_path.clone(),
        smp: args.smp,
        manifest: Some(igvm_manifest_path.clone()),
        output: igvm_path.clone(),
    };
    tracing::warn!("igvm-tools invocation not yet wired — skipping");
    tracing::debug!(cmd = ?igvm_args.to_args(), "would invoke igvm-tools");

    // Step 8: Convert to output format if not raw
    let _final_disk = args.output.join(format!("disk.{}", format_extension(&args.format)));
    tracing::warn!("format conversion not yet implemented — skipping");

    // Step 9: Write manifest
    tracing::warn!("manifest generation not yet implemented — skipping");

    tracing::info!(output = %args.output.display(), "pipeline complete (with stubs)");
    Ok(())
}
