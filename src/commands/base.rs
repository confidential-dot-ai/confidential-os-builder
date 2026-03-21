use std::path::PathBuf;

use crate::{tools, BaseArgs};

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!("building base image");
    tools::require("mkosi")?;

    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    let mkosi_args = &[
        "--directory",
        mkosi_dir.to_str().unwrap(),
        "build",
    ];
    tracing::info!("invoking mkosi {}", mkosi_args.join(" "));
    tools::run_command_streaming("mkosi", mkosi_args)?;
    Ok(())
}
