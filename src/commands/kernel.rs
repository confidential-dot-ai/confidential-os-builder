use crate::KernelArgs;

pub fn run(args: &KernelArgs) -> anyhow::Result<()> {
    tracing::info!(source = %args.source.display(), "building hardened kernel");
    anyhow::bail!("kernel build not yet implemented")
}
