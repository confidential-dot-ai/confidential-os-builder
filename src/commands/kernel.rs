use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::kernel::{compile, config, fetch, manifest as km, version::KernelVersion};
use crate::tools;
use crate::KernelArgs;

const REQUIRED_FRAGMENT: &str = "kernel/required.config";
const HARDENING_FRAGMENT: &str = "kernel/hardening.config";
/// Confidential VM overrides. Merged after `hardening.config` so the last
/// fragment wins — `CONFIG_ACPI_TABLE_UPGRADE=y` here intentionally overrides
/// the `# is not set` line in `hardening.config`. See the file header for the
/// threat-model justification.
const CONFIDENTIAL_FRAGMENT: &str = "kernel/confidential.config";
/// Resolved-config snapshot lockfile. Every kernel build rewrites this with
/// the freshly-resolved `.config`; it's committed to git so `git diff` shows
/// when a fragment edit or kernel bump changed the merged config.
const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const VERSION_PATH: &str = "kernel/version";
const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";
const TOOLS_TREE_CONF: &str = "mkosi/kernel-builder/mkosi.conf";
const TOOLS_TREE_IMAGE: &str = "mkosi/kernel-builder/mkosi.output/image";
const TOOLS_TREE_STAMP: &str = "mkosi/kernel-builder/mkosi.output/.confos-tools-stamp";

pub fn run(args: &KernelArgs) -> Result<()> {
    let version = KernelVersion::read(Path::new(VERSION_PATH))?;
    tracing::info!(linux_version = %version.linux_version, "building hardened kernel");

    // Optional caller-supplied config fragment merged after required +
    // hardening. No flag = confos's bare required + hardening baseline.
    let fragment = args.kernel_config_fragment.as_deref();
    let snapshot = Path::new(SNAPSHOT_PATH);

    fs_err::create_dir_all(&args.output)?;
    let out_dir = args.output.canonicalize()?;
    let cache_dir = out_dir.join("cache");
    let build_dir = out_dir.join("build");
    let log_path = out_dir.join("build.log");
    let vmlinuz_path = out_dir.join("vmlinuz");
    let manifest_path = out_dir.join("manifest.json");

    // Cache short-circuit: skip the entire build if all inputs match and the
    // existing vmlinuz still hashes to what the manifest claims. --force
    // bypasses this.
    if !args.force && manifest_path.exists() && vmlinuz_path.exists() {
        if let Ok(cached) = km::read(&manifest_path) {
            let tools_tree_path = Path::new(TOOLS_TREE_IMAGE);
            if let Ok(live) = compute_fingerprint(&version, tools_tree_path, fragment, snapshot) {
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
    let tools_tree = ensure_tools_tree(args.force, &args.kernel_builder_package)?;

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
        Path::new(CONFIDENTIAL_FRAGMENT),
        fragment,
    )?;

    // Phase 0c.5: refresh the snapshot lockfile. The snapshot auto-updates
    // on every build and never fails it; git tracks the resolved config.
    println!("\n=== Step 0c.5: Updating kernel config snapshot ===");
    let resolved = kernel_src.join(".config");
    if config::update_snapshot(&resolved, snapshot)? {
        println!(
            "snapshot {} updated — review `git diff` and commit it",
            snapshot.display()
        );
    } else {
        println!("snapshot {} unchanged", snapshot.display());
    }

    // Phase 0d: compile
    println!("\n=== Step 0d: Compiling kernel ===");
    fs_err::write(&log_path, b"")?;
    compile::run(&tools_tree, &kernel_src, &vmlinuz_path, &log_path)?;

    // Phase 0e: finalize manifest
    println!("\n=== Step 0e: Writing manifest ===");
    let inputs = compute_fingerprint(&version, &tools_tree, fragment, snapshot)?;
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
fn ensure_tools_tree(force: bool, extra_packages: &[String]) -> Result<PathBuf> {
    let tree = Path::new(TOOLS_TREE_IMAGE);
    let stamp_path = Path::new(TOOLS_TREE_STAMP);
    // Cache key = mkosi.conf hash + the extra-package list. The packages come
    // via flags, not mkosi.conf, so they must be folded in here or a changed
    // --kernel-builder-package list would silently reuse a stale tree.
    let stamp_key = format!(
        "{}\n{}",
        fetch::sha256_file(Path::new(TOOLS_TREE_CONF))?,
        extra_packages.join(",")
    );

    if !force && tree.exists() {
        if let Ok(stamped) = fs_err::read_to_string(stamp_path) {
            if stamped.trim() == stamp_key {
                println!("kernel-builder tools tree cache HIT (mkosi.conf + packages unchanged)");
                return Ok(tree.canonicalize()?);
            }
        }
    }

    // Wipe stale stamp before rebuild so a half-failed `mkosi --force` can't
    // be picked up as a cache hit on the next call.
    let _ = fs_err::remove_file(stamp_path);

    let mkosi = tools::resolve_mkosi()?;
    let mut args: Vec<String> = vec![
        mkosi.clone(),
        "--directory".into(),
        TOOLS_TREE_DIR.into(),
        "--force".into(),
    ];
    for pkg in extra_packages {
        args.push(format!("--package={pkg}"));
    }
    tools::run_command_streaming("sudo", &args)?;
    if !tree.exists() {
        return Err(anyhow!("mkosi did not produce {}", tree.display()));
    }
    fs_err::write(stamp_path, &stamp_key)?;
    Ok(tree.canonicalize()?)
}

/// Compute the fingerprint over all inputs that determine kernel build output.
///
/// `fragment` is the caller-supplied `--kernel-config-fragment` (None when
/// building confos's bare baseline). `snapshot` is the committed snapshot
/// lockfile; hashing it into the fingerprint means a deleted or hand-edited
/// snapshot invalidates the cache and forces a rebuild that regenerates it.
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
        confidential_config_sha256: fetch::sha256_file(Path::new(CONFIDENTIAL_FRAGMENT))?,
        // Hash of the caller's --kernel-config-fragment, empty when none was
        // passed — keeps the fingerprint identical to a bare baseline build.
        kernel_extra_config_sha256: match fragment {
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
