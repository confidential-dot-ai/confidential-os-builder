use std::path::Path;

use crate::compose;
use crate::convert;
use crate::igvm::invoke::IgvmBuildArgs;
use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs,
};
use crate::uki::build::UkifyBuildArgs;
use crate::ImageFormat;

pub struct PipelineArgs {
    pub project_partition: std::path::PathBuf,
    pub kernel: std::path::PathBuf,
    /// When Some, ukify is invoked to build a UKI from kernel+initrd.
    /// When None, kernel is used directly as a prebuilt UKI (ukify step skipped).
    pub initrd: Option<std::path::PathBuf>,
    pub firmware: std::path::PathBuf,
    pub base_image: std::path::PathBuf,
    pub memory: String,
    pub smp: u32,
    pub format: ImageFormat,
    pub output: std::path::PathBuf,
}

pub fn format_extension(format: &ImageFormat) -> &'static str {
    match format {
        ImageFormat::Qcow2 => "qcow2",
        ImageFormat::Vhd => "vhd",
        ImageFormat::Raw => "raw",
    }
}

pub fn run(args: &PipelineArgs) -> anyhow::Result<()> {
    // Stage 5: Compose disk image (base + project)
    let raw_disk = args.output.join("disk.raw");
    compose::disk::compose(&args.base_image, &args.project_partition, &raw_disk)?;
    tracing::info!("disk image composed");

    // Stage 6: Produce UKI — build from kernel+initrd, or copy prebuilt
    let uki_path = args.output.join("uki.efi");
    if let Some(initrd) = &args.initrd {
        let uki_args = UkifyBuildArgs {
            kernel: args.kernel.clone(),
            initrds: vec![initrd.clone()],
            output: uki_path.clone(),
        };
        crate::uki::build::build(&uki_args)?;
        tracing::info!("UKI built via ukify");
    } else {
        fs_err::copy(&args.kernel, &uki_path)?;
        tracing::info!(src = %args.kernel.display(), "using prebuilt UKI");
    }

    // Stage 7: Build IGVM via igvm-tools
    let igvm_work_dir = tempfile::tempdir()?;
    let igvm_manifest_path = igvm_work_dir.path().join("igvm-manifest.json");
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
            initrd: args.initrd.as_ref().map(|p| hash_entry(p)).transpose()?,
            firmware: hash_entry(&args.firmware)?,
            base_image: hash_entry(&args.base_image)?,
            project_partition: hash_entry(&args.project_partition)?,
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
