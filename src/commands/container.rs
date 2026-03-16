use std::path::{Path, PathBuf};

use crate::container as container_helpers;
use crate::mkosi::config::MkosiConfig;
use crate::nftables;
use crate::pipeline::{self, PipelineArgs};
use crate::{tools, ContainerArgs};

fn validate_inputs(args: &ContainerArgs) -> anyhow::Result<()> {
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

pub fn run(args: &ContainerArgs) -> anyhow::Result<()> {
    tracing::info!(url = %args.url, "building container CVM image");

    // Stage 1: Validate inputs
    validate_inputs(args)?;

    // Stage 2: Check required tools
    tools::require("mkosi")?;
    tools::require("ukify")?;
    tools::require("igvm-tools")?;
    tools::require("qemu-img")?;
    tools::require("podman")?;

    // Stage 3: Create output directory
    fs_err::create_dir_all(&args.output)?;

    tracing::info!("all inputs validated and tools found");

    // Stage 4: Build project partition
    let work_dir = tempfile::tempdir()?;

    // 4a: Pull and export OCI image
    container_helpers::pull(&args.url)?;
    let oci_archive = work_dir.path().join("container.oci");
    container_helpers::save(&args.url, &oci_archive)?;
    tracing::info!("container image exported");

    // 4b: Generate mkosi build tree
    let mut mkosi_config = MkosiConfig::container();

    // Postinst scripts: nftables first (index 0), podman second (index 1)
    mkosi_config.add_postinst_script(&nftables::service_rules(args.service_port));
    mkosi_config.add_postinst_script(&container_helpers::podman_postinst());

    // Small extra file: quadlet unit
    mkosi_config.add_extra_file(
        PathBuf::from("etc/containers/systemd/app.container"),
        container_helpers::quadlet(&args.url, args.service_port).into_bytes(),
    );

    // Large extra file: OCI archive — copy directly to avoid loading into memory
    let extra_oci_dir = work_dir.path().join("mkosi.extra/opt/steep");
    fs_err::create_dir_all(&extra_oci_dir)?;
    fs_err::copy(&oci_archive, extra_oci_dir.join("container.oci"))?;

    mkosi_config.invoke(work_dir.path())?;
    let project_partition = work_dir.path().join("project.raw");
    tracing::info!("project partition built");

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
