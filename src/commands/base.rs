use crate::{tools, BaseArgs};

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!(source_image = %args.source_image.display(), "building base image");

    // Validate inputs
    if !args.source_image.exists() {
        anyhow::bail!("source image not found: {}", args.source_image.display());
    }

    // Check required tools
    tools::require("mkosi")?;

    // Create output directory
    fs_err::create_dir_all(&args.output)?;

    // TODO: Generate mkosi config for base image with hardening
    // TODO: Invoke mkosi to build base partition
    // Phase 1 hardening: firewall rules (nftables/iptables)

    tracing::warn!("base image build not yet fully implemented");
    Ok(())
}
