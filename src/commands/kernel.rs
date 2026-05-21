use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::kernel::{compile, config, fetch, manifest as km, version::KernelVersion};
use crate::tools;
use crate::KernelArgs;

const REQUIRED_FRAGMENT: &str = "kernel/required.config";
const HARDENING_FRAGMENT: &str = "kernel/hardening.config";
/// Default snapshot when `--kernel-snapshot` isn't given — steep's own
/// baseline (required + hardening, no extra fragment). A caller that
/// supplies `--kernel-config-fragment` must also point `--kernel-snapshot`
/// at a snapshot generated for that fragment.
pub const DEFAULT_SNAPSHOT: &str = "kernel/config-x86_64.snapshot";
const VERSION_PATH: &str = "kernel/version";
const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";
const TOOLS_TREE_CONF: &str = "mkosi/kernel-builder/mkosi.conf";
const TOOLS_TREE_IMAGE: &str = "mkosi/kernel-builder/mkosi.output/image";
const TOOLS_TREE_STAMP: &str = "mkosi/kernel-builder/mkosi.output/.steep-tools-stamp";

pub fn run(args: &KernelArgs) -> Result<()> {
    let version = KernelVersion::read(Path::new(VERSION_PATH))?;
    tracing::info!(linux_version = %version.linux_version, "building hardened kernel");

    // Resolve caller-supplied kernel inputs. The config fragment is optional
    // (no flag = steep's bare required + hardening baseline). The snapshot
    // defaults to steep's own baseline; a caller passing a fragment must also
    // pass the matching snapshot, since the resolved .config then differs.
    let fragment = args.kernel_config_fragment.as_deref();
    let snapshot: PathBuf = args
        .kernel_snapshot
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SNAPSHOT));
    if fragment.is_some() && args.kernel_snapshot.is_none() {
        return Err(anyhow!(
            "--kernel-config-fragment requires --kernel-snapshot: a fragment \
             changes the resolved .config so it won't match steep's baseline \
             snapshot. Point --kernel-snapshot at the fragment's own snapshot."
        ));
    }

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
            let tools_tree_path = Path::new(TOOLS_TREE_IMAGE);
            if let Ok(live) = compute_fingerprint(&version, tools_tree_path, fragment, &snapshot) {
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
    let tools_tree = ensure_tools_tree(args.force)?;

    // Phase 0b: fetch tarball
    println!("\n=== Step 0b: Fetching kernel tarball ===");
    let tarball = fetch::fetch(&version.linux_version, &version.tarball_sha256, &cache_dir)?;

    // Phase 0c: extract + configure
    println!("\n=== Step 0c: Extracting + configuring kernel ===");
    // The compile/configure phases write into this tree as root via nspawn,
    // so a previous run can leave root-owned files here. force_remove_dir_all
    // falls back to `sudo rm -rf` on EPERM so re-builds always succeed.
    tools::force_remove_dir_all(&build_dir)?;
    fs_err::create_dir_all(&build_dir)?;
    extract_tarball(&tarball, &build_dir)?;
    let kernel_src = build_dir.join(format!("linux-{}", version.linux_version));
    if !kernel_src.exists() {
        return Err(anyhow!(
            "expected extracted dir {} not found",
            kernel_src.display()
        ));
    }

    if let Some(f) = fragment {
        if !f.exists() {
            return Err(anyhow!(
                "--kernel-config-fragment path not found: {}",
                f.display()
            ));
        }
    }
    config::run_configure_phase(
        &tools_tree,
        &kernel_src,
        Path::new(REQUIRED_FRAGMENT),
        Path::new(HARDENING_FRAGMENT),
        fragment,
    )?;

    // Phase 0c.5: snapshot guard
    println!("\n=== Step 0c.5: Snapshot guard ===");
    let resolved = kernel_src.join(".config");
    if args.update_snapshot {
        config::update_snapshot(&resolved, &snapshot)?;
        println!("snapshot updated: {}", snapshot.display());
    } else if !snapshot.exists() {
        return Err(anyhow!(
            "{} does not exist. Generate it with `steep kernel --update-snapshot`.",
            snapshot.display()
        ));
    } else {
        config::check_snapshot(&resolved, &snapshot)?;
    }

    // Phase 0d: compile
    println!("\n=== Step 0d: Compiling kernel ===");
    fs_err::write(&log_path, b"")?;
    compile::run(&tools_tree, &kernel_src, &vmlinuz_path, &log_path)?;

    // Phase 0e: finalize manifest
    println!("\n=== Step 0e: Writing manifest ===");
    let inputs = compute_fingerprint(&version, &tools_tree, fragment, &snapshot)?;
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
///
/// Skips the (slow, sudo-requiring) `mkosi --force` rebuild when a previous
/// build's stamp file matches the current `mkosi.conf` hash. `force` bypasses
/// the skip. The stamp lives under `mkosi.output/`, which `mkosi --force`
/// wipes — so a successful rebuild always lands a fresh stamp, and a failed
/// rebuild leaves no stamp behind to fool a later cache check.
fn ensure_tools_tree(force: bool) -> Result<PathBuf> {
    let tree = Path::new(TOOLS_TREE_IMAGE);
    let stamp_path = Path::new(TOOLS_TREE_STAMP);
    let conf_sha = fetch::sha256_file(Path::new(TOOLS_TREE_CONF))?;

    if !force && tree.exists() {
        if let Ok(stamped) = fs_err::read_to_string(stamp_path) {
            if stamped.trim() == conf_sha {
                println!("kernel-builder tools tree cache HIT (mkosi.conf unchanged)");
                return Ok(tree.canonicalize()?);
            }
        }
    }

    // Wipe stale stamp before rebuild so a half-failed `mkosi --force` can't
    // be picked up as a cache hit on the next call.
    let _ = fs_err::remove_file(stamp_path);

    let mkosi = tools::resolve_mkosi()?;
    tools::run_command_streaming(
        "sudo",
        &[mkosi.as_str(), "--directory", TOOLS_TREE_DIR, "--force"],
    )?;
    if !tree.exists() {
        return Err(anyhow!("mkosi did not produce {}", tree.display()));
    }
    fs_err::write(stamp_path, &conf_sha)?;
    Ok(tree.canonicalize()?)
}

/// Compute the fingerprint over all inputs that determine kernel build output.
///
/// `fragment` is the caller-supplied `--kernel-config-fragment` (None when
/// building steep's bare baseline); `snapshot` is the resolved
/// `--kernel-snapshot` path. Both feed the cache key so switching fragment
/// or snapshot correctly invalidates a cached kernel.
pub fn compute_fingerprint(
    version: &KernelVersion,
    _tools_tree: &Path,
    fragment: Option<&Path>,
    snapshot: &Path,
) -> Result<km::Fingerprint> {
    Ok(km::Fingerprint {
        linux_version: version.linux_version.clone(),
        tarball_sha256: version.tarball_sha256.clone(),
        required_config_sha256: fetch::sha256_file(Path::new(REQUIRED_FRAGMENT))?,
        hardening_config_sha256: fetch::sha256_file(Path::new(HARDENING_FRAGMENT))?,
        // Hash of the caller's --kernel-config-fragment, empty when none was
        // passed — keeps the fingerprint identical to a bare baseline build.
        container_config_sha256: match fragment {
            Some(f) => fetch::sha256_file(f)?,
            None => String::new(),
        },
        snapshot_config_sha256: if snapshot.exists() {
            fetch::sha256_file(snapshot)?
        } else {
            String::new()
        },
        tools_tree_digest: tools_tree_digest()?,
    })
}

/// Hash the toolchain identity. We use the mkosi.conf bytes as a stable proxy:
/// the apt mirror snapshot URL is in there, package list is in there.
fn tools_tree_digest() -> Result<String> {
    fetch::sha256_file(Path::new(TOOLS_TREE_CONF))
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
