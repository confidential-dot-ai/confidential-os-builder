use crate::KernelArgs;

pub fn run(args: &KernelArgs) -> anyhow::Result<()> {
    tracing::info!(source = %args.source.display(), "building hardened kernel");

    // Validate inputs
    if !args.source.exists() {
        anyhow::bail!("kernel source tree not found: {}", args.source.display());
    }
    if !args.config.exists() {
        anyhow::bail!("kernel config not found: {}", args.config.display());
    }

    // Create output directory
    fs_err::create_dir_all(&args.output)?;

    // TODO: Invoke kernel build (make, or wrap a build script)

    tracing::warn!("kernel build not yet implemented");
    Ok(())
}
