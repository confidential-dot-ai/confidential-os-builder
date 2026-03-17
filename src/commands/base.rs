use std::path::PathBuf;

use crate::{tools, BaseArgs};

pub fn run(args: &BaseArgs) -> anyhow::Result<()> {
    tracing::info!("building base image");

    // Step 1: Check required tools
    tools::require("mkosi")?;

    // Step 2: Create output directory
    fs_err::create_dir_all(&args.output)?;

    // Step 3: Invoke mkosi against static config folder
    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    let output_dir = tempfile::tempdir()?;
    tracing::info!(config = %mkosi_dir.display(), "invoking mkosi");
    tools::run_command_streaming("mkosi", &[
        "--directory",
        mkosi_dir.to_str().unwrap(),
        "--output-dir",
        output_dir.path().to_str().unwrap(),
        "build",
    ])?;

    // Step 4: Copy mkosi output to args.output/base.raw
    let mkosi_output = output_dir.path().join("image.raw");
    let dest = args.output.join("base.raw");
    fs_err::copy(&mkosi_output, &dest)?;
    tracing::info!(dest = %dest.display(), "base image written");

    tracing::info!(output = %args.output.display(), "base image build complete");
    Ok(())
}
