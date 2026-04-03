use std::path::PathBuf;

use crate::{tools, BaseArgs};

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!("building base image");
    tools::require("mkosi")?;

    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    let mkosi_dir_str = mkosi_dir.to_string_lossy();
    let mut mkosi_args: Vec<&str> = vec![
        "--directory",
        &mkosi_dir_str,
        "build",
    ];

    if args.force {
        mkosi_args.push("--force");
    }

    tracing::info!("invoking mkosi {}", mkosi_args.join(" "));
    tools::run_command_streaming("mkosi", &mkosi_args)?;

    Ok(())
}
