use std::path::{Path, PathBuf};

use crate::{tools, CloudInitArgs};
use crate::pipeline::{self, PipelineArgs};

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Stage 1: Validate inputs
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    ensure_file_exists(&args.kernel, "kernel")?;
    if let Some(initrd) = &args.initrd {
        ensure_file_exists(initrd, "initrd")?;
    }
    ensure_file_exists(&args.firmware, "firmware")?;
    ensure_file_exists(&args.base_image, "base image")?;

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

    // Stage 4: Build cidata partition via mkosi
    let mkosi_dir = PathBuf::from("mkosi/cidata");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    let work_dir = tempfile::tempdir()?;
    tracing::info!(config = %mkosi_dir.display(), "invoking mkosi for cidata partition");
    tools::run_command_streaming("mkosi", &[
        "--directory",
        mkosi_dir.to_str().unwrap(),
        "--output-dir",
        work_dir.path().to_str().unwrap(),
        "--extra-trees",
        args.dir.to_str().unwrap(),
        "build",
    ])?;

    let project_partition = work_dir.path().join("image.raw");
    tracing::info!("cidata partition built");

    // Stages 5-9: Shared pipeline
    pipeline::run(&PipelineArgs {
        project_partition,
        kernel: args.kernel.clone(),
        initrd: args.initrd.clone(),
        firmware: args.firmware.clone(),
        base_image: args.base_image.clone(),
        memory: args.memory.clone(),
        smp: args.smp,
        format: args.format.clone(),
        output: args.output.clone(),
    })
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
