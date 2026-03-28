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
    build_cidata_partition(&args.dir, &output_dir)?;

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
fn build_cidata_partition(cloud_init_dir: &Path, output: &Path) -> anyhow::Result<()> {
    // Collect files from the cloud-init directory
    let entries: Vec<_> = fs_err::read_dir(cloud_init_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .map(|f| f.file_name().into_string().unwrap())
        .collect();

    if entries.is_empty() {
        anyhow::bail!(
            "no files found in cloud-init directory: {}",
            cloud_init_dir.display()
        );
    }

    let ci_path = output.join("seed.iso");
    let ci_path_str = &ci_path.to_string_lossy();
    let mut ci_args = vec![
        "-output",
        ci_path_str,
        "-input-charset",
        "utf-8",
        "-volid",
        "cidata",
        "-joliet",
        "-rock",
    ];
    for e in &entries {
        ci_args.push(e);
    }
    tools::run_command_streaming_in("genisoimage", &ci_args, cloud_init_dir.to_owned())?;
    tracing::info!("cidata partition built");

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
