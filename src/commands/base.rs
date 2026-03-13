use crate::BaseArgs;

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!(source_image = %args.source_image.display(), "building base image");
    anyhow::bail!("base image build not yet implemented")
}
