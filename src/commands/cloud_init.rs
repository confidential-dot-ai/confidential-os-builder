use crate::CloudInitArgs;

pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");
    anyhow::bail!("cloud-init build not yet implemented")
}
