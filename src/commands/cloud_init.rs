use std::path::{Path, PathBuf};

use crate::{tools, CloudInitArgs};

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");
    let project_name = args.dir.file_name()
        .ok_or_else(|| anyhow::anyhow!("cloud-init directory has no file name: {}", args.dir.display()))?;

    ensure_dir_exists(&args.dir, "cloud-init directory")?;
    tools::require("qemu-img")?;
    tools::require("genisoimage")?;
    tracing::info!("all inputs validated and tools found");

    let output_dir = PathBuf::from("output").join(project_name);
    if fs_err::exists(&output_dir)? {
        fs_err::remove_dir_all(&output_dir)?;
    }
    fs_err::create_dir_all(&output_dir)?;
    let output_dir = output_dir.canonicalize()?;

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

    println!("Image created in {}", output_dir.to_string_lossy());
    Ok(())
}

/// Build a vfat cidata partition image from cloud-init config files.
fn build_cidata_partition(cloud_init_dir: &Path, output: &Path) -> anyhow::Result<()> {
    // Collect files from the cloud-init directory
    let entries: Vec<String> = fs_err::read_dir(cloud_init_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .filter_map(|f| f.file_name().into_string().ok())
        .collect();

    if entries.is_empty() {
        anyhow::bail!(
            "no files found in cloud-init directory: {}",
            cloud_init_dir.display()
        );
    }

    if !entries.contains(&"user-data".to_owned()) {
        anyhow::bail!("no user-data file found in {}", cloud_init_dir.display())
    }

    // Stage files into a temp dir so we don't mutate the user's source directory
    let staging = tempfile::tempdir()?;
    for entry in fs_err::read_dir(cloud_init_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs_err::copy(entry.path(), staging.path().join(entry.file_name()))?;
        }
    }

    // Ensure we have a minimal meta-data so cloud-init will run
    if !entries.contains(&"meta-data".to_owned()) {
        let instance_id = cloud_init_dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "steep".to_string());
        fs_err::write(
            staging.path().join("meta-data"),
            format!("instance-id: {instance_id}"),
        )?;
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
    if !entries.contains(&"meta-data".to_owned()) {
        ci_args.push("meta-data");
    }
    tools::run_command_streaming_in("genisoimage", &ci_args, staging.path().to_owned())?;
    tracing::info!("cidata partition built");

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
