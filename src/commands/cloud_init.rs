use std::path::Path;

use crate::compose;
use crate::convert;
use crate::igvm::invoke::IgvmBuildArgs;
use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs,
};
use crate::mkosi::config::MkosiConfig;
use crate::nftables;
use crate::uki::build::UkifyBuildArgs;
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

    // Stage 1: Validate inputs
    validate_inputs(args)?;

    // Stage 2: Check required tools
    tools::require("mkosi")?;
    tools::require("ukify")?;
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
    let project_partition = work_dir.path().join("project.raw");
    tracing::info!("project partition built");

    // Stage 5: Compose disk image (base + project)
    let raw_disk = args.output.join("disk.raw");
    compose::disk::compose(&args.base_image, &project_partition, &raw_disk)?;
    tracing::info!("disk image composed");

    // Stage 6: Build UKI via ukify
    let uki_path = args.output.join("uki.efi");
    let uki_args = UkifyBuildArgs {
        kernel: args.kernel.clone(),
        initrds: vec![args.initrd.clone()],
        output: uki_path.clone(),
    };
    crate::uki::build::build(&uki_args)?;
    tracing::info!("UKI built");

    // Stage 7: Build IGVM via igvm-tools
    let igvm_manifest_path = work_dir.path().join("igvm-manifest.json");
    let igvm_path = args.output.join("guest.igvm");
    let igvm_args = IgvmBuildArgs {
        firmware: args.firmware.clone(),
        kernel: uki_path.clone(),
        smp: args.smp,
        manifest: Some(igvm_manifest_path.clone()),
        output: igvm_path.clone(),
    };
    crate::igvm::invoke::build(&igvm_args)?;
    tracing::info!("IGVM built");

    // Stage 8: Convert to output format
    let final_disk = args.output.join(format!("disk.{}", format_extension(&args.format)));
    convert::convert(&raw_disk, &final_disk, &args.format)?;
    // Remove raw intermediate if we converted to another format
    if !matches!(args.format, ImageFormat::Raw) && raw_disk.exists() {
        fs_err::remove_file(&raw_disk)?;
    }
    tracing::info!(format = format_extension(&args.format), "disk image ready");

    // Stage 9: Write manifest
    let igvm_manifest_json = fs_err::read_to_string(&igvm_manifest_path)?;
    let measurement = manifest::parse_igvm_manifest(&igvm_manifest_json)?;

    let build_manifest = BuildManifest {
        version: 1,
        build: BuildConfig {
            timestamp: chrono_now(),
            smp: args.smp,
            memory: args.memory.clone(),
            format: format_extension(&args.format).to_string(),
            platform: "snp".to_string(),
        },
        inputs: ManifestInputs {
            kernel: hash_entry(&args.kernel)?,
            initrd: hash_entry(&args.initrd)?,
            firmware: hash_entry(&args.firmware)?,
            base_image: hash_entry(&args.base_image)?,
            project_partition: hash_entry(&project_partition)?,
        },
        outputs: ManifestOutputs {
            disk_image: hash_entry(&final_disk)?,
            igvm: hash_entry(&igvm_path)?,
            uki: hash_entry(&uki_path)?,
        },
        measurement,
    };

    let manifest_path = args.output.join("manifest.json");
    manifest::write_manifest(&build_manifest, &manifest_path)?;
    tracing::info!(path = %manifest_path.display(), "manifest written");

    tracing::info!(output = %args.output.display(), "pipeline complete");
    Ok(())
}

fn hash_entry(path: &Path) -> anyhow::Result<FileEntry> {
    Ok(FileEntry {
        path: path.display().to_string(),
        sha256: manifest::sha256_file(path)?,
    })
}

fn chrono_now() -> String {
    let output = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}
