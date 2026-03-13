use crate::ContainerArgs;

pub fn run(args: &ContainerArgs) -> anyhow::Result<()> {
    tracing::info!(url = %args.url, "building container CVM image");
    anyhow::bail!("container build not yet implemented")
}
