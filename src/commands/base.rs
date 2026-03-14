use crate::{tools, BaseArgs};
use std::path::Path;

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!(source_image = %args.source_image, "building base image");

    let source_path = Path::new(&args.source_image);
    if !source_path.exists() {
        anyhow::bail!("source image not found: {}", args.source_image);
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
