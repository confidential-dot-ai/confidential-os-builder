use crate::{nftables, tools, BaseArgs};
use crate::mkosi::config::MkosiConfig;

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!("building base image");

    // Step 1: Check required tools
    tools::require("mkosi")?;

    // Step 2: Create output directory
    fs_err::create_dir_all(&args.output)?;

    // Step 3: Generate mkosi config
    let work_dir = tempfile::tempdir()?;
    let mut config = MkosiConfig::base();

    // Step 5: Add nftables hardening (block all traffic)
    config.add_postinst_script(&nftables::base_rules());

    // Step 6: Invoke mkosi
    config.invoke(work_dir.path())?;

    // Step 7: Copy mkosi output to args.output/base.raw
    let mkosi_output = work_dir.path().join("image.raw");
    let dest = args.output.join("base.raw");
    fs_err::copy(&mkosi_output, &dest)?;
    tracing::info!(dest = %dest.display(), "base image written");

    tracing::info!(output = %args.output.display(), "base image build complete");
    Ok(())
}
