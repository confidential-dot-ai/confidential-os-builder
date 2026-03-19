use std::path::Path;

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
    tools::require("mkfs.vfat")?;
    if args.initrd.is_some() {
        tools::require("ukify")?;
    }
    tools::require("igvm-tools")?;
    tools::require("qemu-img")?;

    // Stage 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Stage 4: Build cidata partition (vfat with cloud-init files)
    let work_dir = tempfile::tempdir()?;
    let project_partition = work_dir.path().join("image.raw");
    build_cidata_partition(&args.dir, &project_partition)?;
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

/// Build a vfat cidata partition image from cloud-init config files.
fn build_cidata_partition(cloud_init_dir: &Path, output: &Path) -> anyhow::Result<()> {
    // Collect files from the cloud-init directory
    let entries: Vec<_> = fs_err::read_dir(cloud_init_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .collect();

    if entries.is_empty() {
        anyhow::bail!(
            "no files found in cloud-init directory: {}",
            cloud_init_dir.display()
        );
    }

    // Calculate size: 8MB minimum, enough for all files
    let total_size: u64 = entries
        .iter()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    let image_size = std::cmp::max(8 * 1024 * 1024, (total_size + 1024 * 1024) & !0xFFFFF);

    // Create empty image file
    let f = fs_err::File::create(output)?;
    f.set_len(image_size)?;
    drop(f);

    // Format as vfat with label "cidata"
    tools::run_command_streaming("mkfs.vfat", &["-n", "cidata", &output.display().to_string()])?;

    // Mount and copy files
    let mount_dir = tempfile::tempdir()?;
    tools::run_command_streaming(
        "mount",
        &[
            "-o",
            "loop",
            &output.display().to_string(),
            &mount_dir.path().display().to_string(),
        ],
    )?;

    let mount_path = mount_dir.path().to_path_buf();
    let result = (|| -> anyhow::Result<()> {
        for entry in &entries {
            let dest = mount_path.join(entry.file_name());
            fs_err::copy(entry.path(), &dest)?;
            tracing::debug!(file = %entry.file_name().to_string_lossy(), "copied to cidata");
        }
        Ok(())
    })();

    // Always unmount, even if copy failed
    let umount_result =
        tools::run_command_streaming("umount", &[&mount_dir.path().display().to_string()]);

    result?;
    umount_result?;

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
