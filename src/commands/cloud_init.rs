use std::path::{Path, PathBuf};

use crate::{tools, CloudInitArgs};

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

    // Stage 1: Validate inputs
    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    // ensure_file_exists(&args.kernel, "kernel")?;
    // if let Some(initrd) = &args.initrd {
    //     ensure_file_exists(initrd, "initrd")?;
    // }
    // ensure_file_exists(&args.firmware, "firmware")?;
    // ensure_file_exists(&args.base_image, "base image")?;

    // Stage 2: Check required tools
    // tools::require("mkfs.vfat")?;
    // if args.initrd.is_some() {
    //     tools::require("ukify")?;
    // }
    // tools::require("igvm-tools")?;
    tools::require("qemu-img")?;
    tracing::info!("all inputs validated and tools found");

    // create the output directory
    let mut output_dir = PathBuf::new();
    output_dir.push("output");
    output_dir.push(args.dir.file_name().unwrap());
    let output_dir = output_dir.canonicalize()?;
    fs_err::create_dir_all(&output_dir)?;

    // Build cidata ISO
    let ci_path = output_dir.join("seed.iso");
    let ci_path_str = &ci_path.to_string_lossy();
    let ci_args = vec![
        "-output",
        ci_path_str,
        "-input-charset",
        "utf-8",
        "-volid",
        "cidata",
        "-joliet",
        "-rock",
        "user-data",
        "meta-data",
    ];
    tools::run_command_streaming_in("genisoimage", &ci_args, Some(args.dir.to_owned()))?;
    tracing::info!("cidata partition built");

    // Create the qcow2
    let img_path = output_dir.join("image.qcow2");
    let img_path_str = &img_path.to_string_lossy();
    let base_path = std::env::current_dir()?.join("mkosi/base/mkosi.output/image.raw");
    let base_path_str = base_path.to_string_lossy();
    let img_args = vec![
        "create",
        "-B",
        "raw",
        "-b",
        &base_path_str,
        "-f",
        "qcow2",
        img_path_str,
        "5G",
    ];
    tools::run_command_streaming("qemu-img", &img_args)?;
    tracing::info!("qcow2 image created at {}", img_path_str);

    Ok(())
}

/// Build a vfat cidata partition image from cloud-init config files.
fn _build_cidata_partition(cloud_init_dir: &Path, output: &Path) -> anyhow::Result<()> {
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
    tools::run_command_streaming(
        "mkfs.vfat",
        &["-n", "cidata", &output.display().to_string()],
    )?;

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

fn _ensure_file_exists(path: &Path, label: &str) -> anyhow::Result<()> {
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
