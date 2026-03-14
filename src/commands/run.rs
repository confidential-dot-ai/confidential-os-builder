use crate::RunArgs;

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "launching CVM");
    anyhow::bail!("run subcommand not yet implemented")
}
