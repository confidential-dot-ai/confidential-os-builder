use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{tools, PublishArgs};

pub fn run(args: &PublishArgs) -> anyhow::Result<()> {
    let disk_path = args.dir.join("disk.raw");
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.raw not found in {}. Run `steep seal` first.",
            args.dir.display()
        );
    }

    let tag = match &args.tag {
        Some(t) => t.clone(),
        None => tag_from_manifest(&args.dir).unwrap_or_else(|| "latest".to_string()),
    };

    let base = format!("{}/{}", args.registry, args.name);
    let image_ref = format!("{base}:{tag}");
    let container_tool = find_container_tool()?;

    // KubeVirt containerDisk expects the disk image at /disk/<name>.
    let disk_abs = disk_path.canonicalize()?;
    let build_ctx = disk_abs.parent().unwrap();
    let dockerfile = build_ctx.join("Dockerfile.containerDisk");
    fs_err::write(&dockerfile, "FROM scratch\nCOPY disk.raw /disk/\n")?;
    let _guard = CleanupFile(dockerfile.clone());

    let size = humansize::format_size(fs_err::metadata(&disk_abs)?.len(), humansize::BINARY);
    println!("Building containerDisk image: {image_ref}");
    println!("  disk: {} ({size})", disk_abs.display());

    tools::run_command_streaming(
        container_tool,
        &[
            "build",
            "-t",
            &image_ref,
            "-f",
            &dockerfile.to_string_lossy(),
            &build_ctx.to_string_lossy(),
        ],
    )?;

    // Collect all refs: the primary tag, plus "latest" if we used a content-addressed tag.
    let mut refs = vec![image_ref];
    if tag != "latest" {
        let latest_ref = format!("{base}:latest");
        tools::run_command_streaming(container_tool, &["tag", &refs[0], &latest_ref])?;
        println!("Tagged: {latest_ref}");
        refs.push(latest_ref);
    }

    // Build IGVM companion image if --igvm flag is set and files exist.
    let igvm_files = if args.igvm { find_igvm_files(&args.dir) } else { Vec::new() };
    let igvm_refs = if !igvm_files.is_empty() {
        let igvm_base = format!("{}/{}-igvm", args.registry, args.name);
        let igvm_ref = format!("{igvm_base}:{tag}");

        let mut dockerfile_content = String::from("FROM scratch\n");
        for f in &igvm_files {
            let name = f.file_name().unwrap().to_string_lossy();
            dockerfile_content.push_str(&format!("COPY {name} /igvm/\n"));
        }

        let igvm_dockerfile = build_ctx.join("Dockerfile.igvm");
        fs_err::write(&igvm_dockerfile, &dockerfile_content)?;
        let _igvm_guard = CleanupFile(igvm_dockerfile.clone());

        let count = igvm_files.len();
        println!("\nBuilding IGVM artifact image: {igvm_ref}");
        println!("  files: {count} IGVM files");

        tools::run_command_streaming(
            container_tool,
            &[
                "build",
                "-t",
                &igvm_ref,
                "-f",
                &igvm_dockerfile.to_string_lossy(),
                &build_ctx.to_string_lossy(),
            ],
        )?;

        let mut igvm_refs = vec![igvm_ref];
        if tag != "latest" {
            let igvm_latest = format!("{igvm_base}:latest");
            tools::run_command_streaming(container_tool, &["tag", &igvm_refs[0], &igvm_latest])?;
            println!("Tagged: {igvm_latest}");
            igvm_refs.push(igvm_latest);
        }
        igvm_refs
    } else {
        Vec::new()
    };

    if args.push {
        for r in refs.iter().chain(igvm_refs.iter()) {
            println!("Pushing {r}");
            tools::run_command_streaming(container_tool, &["push", r])?;
            println!("Published: {r}");
        }
    } else {
        println!("\nImage built locally: {}", refs[0]);
        if !igvm_refs.is_empty() {
            println!("IGVM image built locally: {}", igvm_refs[0]);
        }
        println!("Run with --push to push to registry.");
    }

    Ok(())
}

fn tag_from_manifest(dir: &Path) -> Option<String> {
    let content = fs_err::read_to_string(dir.join("manifest.json")).ok()?;
    let manifest: Value = serde_json::from_str(&content).ok()?;
    let hash = manifest["outputs"]["disk_image"]["sha256"].as_str()?;
    Some(format!("sha-{hash}"))
}

fn find_container_tool() -> anyhow::Result<&'static str> {
    if tools::require("docker").is_ok() {
        Ok("docker")
    } else if tools::require("podman").is_ok() {
        Ok("podman")
    } else {
        anyhow::bail!("neither docker nor podman found in PATH")
    }
}

fn find_igvm_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map_or(false, |ext| ext == "igvm")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("guest-smp")
        })
        .collect();
    files.sort();
    files
}

struct CleanupFile(PathBuf);

impl Drop for CleanupFile {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.0);
    }
}
