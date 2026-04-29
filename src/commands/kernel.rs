use crate::KernelArgs;

pub fn run(args: &KernelArgs) -> anyhow::Result<()> {
    let _ = (args.force, args.update_snapshot, &args.output);
    tracing::warn!("kernel build not yet implemented");
    Ok(())
}
