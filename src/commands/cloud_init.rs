use std::path::Path;

use crate::mkosi::config::MkosiConfig;
use crate::nftables;
use crate::pipeline::{self, PipelineArgs};
use crate::{tools, CloudInitArgs};

fn validate_inputs(args: &CloudInitArgs) -> anyhow::Result<()> {
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    ensure_file_exists(&args.kernel, "kernel")?;
    if let Some(initrd) = &args.initrd {
        ensure_file_exists(initrd, "initrd")?;
    }
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

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Stage 1: Validate inputs
    validate_inputs(args)?;

    // Stage 2: Check required tools
    tools::require("mkosi")?;
    if args.initrd.is_some() {
        tools::require("ukify")?;
    }
    tools::require("igvm-tools")?;
    tools::require("qemu-img")?;

    // Stage 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Stage 4: Build project partition via mkosi
    let work_dir = tempfile::tempdir()?;
    let mut mkosi_config = MkosiConfig::cloud_init(args.dir.clone());
    mkosi_config.add_postinst_script(&nftables::service_rules(args.service_port));
    mkosi_config.invoke(work_dir.path())?;
    let project_partition = work_dir.path().join("image.raw");
    tracing::info!("project partition built");

    // Stages 5-9: Shared pipeline
    pipeline::run(&PipelineArgs {
        project_partition,
        kernel: args.kernel.clone(),
        initrd: args.initrd.clone(),  // None → kernel is prebuilt UKI
        firmware: args.firmware.clone(),
        base_image: args.base_image.clone(),
        memory: args.memory.clone(),
        smp: args.smp,
        format: args.format.clone(),
        output: args.output.clone(),
    })
}
