use std::ffi::OsString;

use crate::{tools, PullArgs};

pub fn run(args: &PullArgs) -> anyhow::Result<()> {
    tools::require("oras")?;

    let output_dir = std::path::Path::new("output").join(&args.name);

    if output_dir.exists() {
        anyhow::bail!(
            "output directory already exists: {}. Remove it first.",
            output_dir.display()
        );
    }

    fs_err::create_dir_all(&output_dir)?;

    let image_ref = format!("{}/{}:{}", args.registry, args.name, args.tag);
    println!("Pulling {} into {}", image_ref, output_dir.display());

    let oras_args: Vec<OsString> = vec![
        "pull".into(),
        image_ref.into(),
    ];

    let output_dir = output_dir
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to resolve output directory: {}", e))?;

    tools::run_command_streaming_in("oras", &oras_args, output_dir)?;

    println!("Pulled successfully.");
    Ok(())
}
