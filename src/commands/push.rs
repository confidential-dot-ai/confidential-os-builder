use std::ffi::OsString;

use crate::{tools, PushArgs};

pub fn run(args: &PushArgs) -> anyhow::Result<()> {
    tools::require("oras")?;

    let dir = args
        .dir
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("directory not found: {}", args.dir.display()))?;

    let disk_path = dir.join("disk.raw");
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.raw not found in {}. Run `steep build` first.",
            dir.display()
        );
    }

    // Collect all regular files, skipping symlinks
    let mut files: Vec<OsString> = Vec::new();
    for entry in fs_err::read_dir(&dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            files.push(entry.file_name());
        }
    }
    files.sort();

    if files.is_empty() {
        anyhow::bail!("no files found in {}", dir.display());
    }

    let name = args
        .name
        .clone()
        .unwrap_or_else(|| args.dir.file_name().unwrap().to_string_lossy().to_string());
    let image_ref = format!("{}/{}:{}", args.registry, name, args.tag);
    println!("Pushing {} files to {}", files.len(), image_ref);
    for f in &files {
        println!("  {}", f.to_string_lossy());
    }

    let mut oras_args: Vec<OsString> = vec![
        "push".into(),
        image_ref.into(),
        "--artifact-type".into(),
        "application/vnd.steep.image.v1".into(),
    ];
    oras_args.extend(files);

    tools::run_command_streaming_in("oras", &oras_args, dir)?;

    println!("Pushed successfully.");
    Ok(())
}
