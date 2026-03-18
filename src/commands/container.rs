use crate::container as container_helpers;
use crate::commands::cloud_init;
use crate::{CloudInitArgs, ContainerArgs};

pub fn run(args: &ContainerArgs) -> anyhow::Result<()> {
    tracing::info!(url = %args.url, "building container CVM image");

    // Generate cloud-init directory
    let cloud_init_dir = tempfile::tempdir()?;
    fs_err::write(
        cloud_init_dir.path().join("user-data"),
        container_helpers::user_data(&args.url, args.service_port),
    )?;
    fs_err::write(
        cloud_init_dir.path().join("meta-data"),
        container_helpers::meta_data(),
    )?;
    tracing::info!(dir = %cloud_init_dir.path().display(), "generated cloud-init directory");

    // Delegate to cloud-init command
    cloud_init::run(&CloudInitArgs {
        dir: cloud_init_dir.path().to_path_buf(),
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
