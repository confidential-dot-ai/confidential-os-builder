use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::kernel::{compile, config, fetch, manifest as km, version::KernelVersion};
use crate::tools;
use crate::KernelArgs;

const REQUIRED_FRAGMENT: &str = "kernel/required.config";
const HARDENING_FRAGMENT: &str = "kernel/hardening.config";
const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const VERSION_PATH: &str = "kernel/version";
const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";

pub fn run(args: &KernelArgs) -> Result<()> {
    let version = KernelVersion::read(Path::new(VERSION_PATH))?;
    tracing::info!(linux_version = %version.linux_version, "building hardened kernel");

    fs_err::create_dir_all(&args.output)?;
    let out_dir = args.output.canonicalize()?;
    let cache_dir = out_dir.join("cache");
    let build_dir = out_dir.join("build");
    let log_path = out_dir.join("build.log");
    let vmlinuz_path = out_dir.join("vmlinuz");
    let manifest_path = out_dir.join("manifest.json");

    // Cache short-circuit: skip the entire build if all inputs match and the
    // existing vmlinuz still hashes to what the manifest claims. --force and
    // --update-snapshot both bypass this.
    if !args.force && !args.update_snapshot && manifest_path.exists() && vmlinuz_path.exists() {
        if let Ok(cached) = km::read(&manifest_path) {
            let tools_tree_path = Path::new("mkosi/kernel-builder/mkosi.output/image");
            if let Ok(live) = compute_fingerprint(&version, tools_tree_path) {
                if cached.inputs == live {
                    let actual = fetch::sha256_file(&vmlinuz_path)?;
                    if actual.eq_ignore_ascii_case(&cached.outputs.vmlinuz_sha256) {
                        println!(
                            "kernel cache HIT (linux {}, sha256 {})",
                            cached.linux_version, actual
                        );
                        return Ok(());
                    }
                    return Err(anyhow!(
                        "kernel artifact corrupted (sha256 mismatch). Re-run with --force."
                    ));
                }
            }
        }
    }

    // Phase 0a: ensure tools tree
    println!("\n=== Step 0a: Ensuring kernel-builder tools tree (mkosi) ===");
    let tools_tree = ensure_tools_tree()?;

    // Phase 0b: fetch tarball
    println!("\n=== Step 0b: Fetching kernel tarball ===");
    let tarball = fetch::fetch(&version.linux_version, &version.tarball_sha256, &cache_dir)?;

    // Phase 0c: extract + configure
    println!("\n=== Step 0c: Extracting + configuring kernel ===");
    if build_dir.exists() {
        fs_err::remove_dir_all(&build_dir)?;
    }
    fs_err::create_dir_all(&build_dir)?;
    extract_tarball(&tarball, &build_dir)?;
    let kernel_src = build_dir.join(format!("linux-{}", version.linux_version));
    if !kernel_src.exists() {
        return Err(anyhow!(
            "expected extracted dir {} not found",
            kernel_src.display()
        ));
    }

    config::run_configure_phase(
        &tools_tree,
        &kernel_src,
        Path::new(REQUIRED_FRAGMENT),
        Path::new(HARDENING_FRAGMENT),
    )?;

    // Phase 0c.5: snapshot guard
    println!("\n=== Step 0c.5: Snapshot guard ===");
    let resolved = kernel_src.join(".config");
    let snapshot = Path::new(SNAPSHOT_PATH);
    if args.update_snapshot {
        config::update_snapshot(&resolved, snapshot)?;
        println!("snapshot updated: {}", snapshot.display());
    } else if !snapshot.exists() {
        return Err(anyhow!(
            "{} does not exist. Generate it with `steep kernel --update-snapshot`.",
            snapshot.display()
        ));
    } else {
        config::check_snapshot(&resolved, snapshot)?;
    }

    // Phase 0d: compile
    println!("\n=== Step 0d: Compiling kernel ===");
    fs_err::write(&log_path, b"")?;
    compile::run(&tools_tree, &kernel_src, &vmlinuz_path, &log_path)?;

    // Phase 0e: finalize manifest
    println!("\n=== Step 0e: Writing manifest ===");
    let inputs = compute_fingerprint(&version, &tools_tree)?;
    let outputs = km::Outputs {
        vmlinuz_sha256: fetch::sha256_file(&vmlinuz_path)?,
    };
    let manifest = km::KernelManifest {
        version: 1,
        linux_version: version.linux_version.clone(),
        inputs,
        outputs,
        built_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    km::write(&manifest_path, &manifest)?;
    println!("kernel: {}", vmlinuz_path.display());
    println!("manifest: {}", manifest_path.display());
    Ok(())
}

/// Build the kernel-builder tools tree if needed, return its path.
fn ensure_tools_tree() -> Result<PathBuf> {
    let tree = Path::new(TOOLS_TREE_DIR).join("mkosi.output/image");
    let mkosi = tools::resolve_mkosi()?;
    tools::run_command_streaming(
        "sudo",
        &[mkosi.as_str(), "--directory", TOOLS_TREE_DIR, "--force"],
    )?;
    if !tree.exists() {
        return Err(anyhow!("mkosi did not produce {}", tree.display()));
    }
    Ok(tree.canonicalize()?)
}

/// Compute the fingerprint over all inputs that determine kernel build output.
pub fn compute_fingerprint(version: &KernelVersion, _tools_tree: &Path) -> Result<km::Fingerprint> {
    Ok(km::Fingerprint {
        linux_version: version.linux_version.clone(),
        tarball_sha256: version.tarball_sha256.clone(),
        required_config_sha256: fetch::sha256_file(Path::new(REQUIRED_FRAGMENT))?,
        hardening_config_sha256: fetch::sha256_file(Path::new(HARDENING_FRAGMENT))?,
        snapshot_config_sha256: if Path::new(SNAPSHOT_PATH).exists() {
            fetch::sha256_file(Path::new(SNAPSHOT_PATH))?
        } else {
            String::new()
        },
        tools_tree_digest: tools_tree_digest()?,
    })
}

/// Hash the toolchain identity. We use the mkosi.conf bytes as a stable proxy:
/// the apt mirror snapshot URL is in there, package list is in there.
fn tools_tree_digest() -> Result<String> {
    fetch::sha256_file(Path::new("mkosi/kernel-builder/mkosi.conf"))
}

fn extract_tarball(tarball: &Path, dest: &Path) -> Result<()> {
    tools::run_command_streaming(
        "tar",
        &[
            "--extract",
            "--xz",
            "--file",
            &tarball.to_string_lossy(),
            "--directory",
            &dest.to_string_lossy(),
        ],
    )?;
    Ok(())
}
