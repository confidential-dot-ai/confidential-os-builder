//! The kernel-builder tools tree: its inputs, rebuild stamp and mkosi call.
//!
//! The tree's content is decided by `mkosi/kernel-builder/mkosi.conf`, its
//! `mkosi.sandbox` and this file alone. CI's tools cache keys hash exactly
//! those paths, so `kernel.rs` can change without forcing a tools rebuild;
//! anything that changes what lands in the tree belongs here.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};

use crate::kernel::fetch;
use crate::tools;

const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";
const TOOLS_TREE_CONF: &str = "mkosi/kernel-builder/mkosi.conf";
const TOOLS_TREE_SANDBOX: &str = "mkosi/kernel-builder/mkosi.sandbox";
pub(super) const TOOLS_TREE_OUTPUT: &str = "mkosi/kernel-builder/mkosi.output";
/// The ACPI harness's tools tree: the production packages plus QEMU, Python
/// and a static libc. It lives beside the production tree so neither
/// invalidates the other, and the harness packages never enter the measured
/// toolchain.
pub(super) const HARNESS_TOOLS_TREE_OUTPUT: &str = "mkosi/kernel-builder/mkosi.output-acpi";
pub(super) const HARNESS_TOOLS_PACKAGES: [&str; 3] = ["libc6-dev", "python3", "qemu-system-x86"];

/// Build the kernel-builder tools tree under `output` if needed, return
/// its image path.
///
/// Skips the (slow, sudo-requiring) `mkosi --force` rebuild when a previous
/// build's stamp file matches the current `mkosi.conf` hash. `force` bypasses
/// the skip. The stamp lives under `output`, which `mkosi --force`
/// wipes — so a successful rebuild always lands a fresh stamp, and a failed
/// rebuild leaves no stamp behind to fool a later cache check.
pub(super) fn ensure_tools_tree(
    force: bool,
    extra_packages: &[String],
    output: &Path,
) -> Result<PathBuf> {
    let tree = output.join("image");
    let stamp_path = output.join(".confos-tools-stamp");
    // Cache key = the tools-tree inputs digest + the extra-package list.
    // The packages come via flags, not mkosi.conf, so they must be folded in
    // here or a changed --kernel-builder-package list would silently reuse a
    // stale tree.
    let stamp_key = format!(
        "{}\n{}",
        tools_tree_inputs_digest()?,
        extra_packages.join(",")
    );

    if !force && tree.exists() {
        if let Ok(stamped) = fs_err::read_to_string(&stamp_path) {
            if stamped == stamp_key {
                println!("kernel-builder tools tree cache HIT (mkosi.conf + packages unchanged)");
                return Ok(tree.canonicalize()?);
            }
        }
    }

    // Wipe stale stamp before rebuild so a half-failed `mkosi --force` can't
    // be picked up as a cache hit on the next call.
    let _ = fs_err::remove_file(&stamp_path);

    // mkosi chdirs into --directory before reading paths, so hand it the
    // output directory absolute. Create it first: mkosi runs under sudo and
    // would otherwise own a fresh directory as root, and the stamp below is
    // written unprivileged.
    fs_err::create_dir_all(output)?;
    let output_abs = std::env::current_dir()?.join(output);
    let mut args: Vec<String> = vec![
        "--directory".into(),
        TOOLS_TREE_DIR.into(),
        format!("--output-directory={}", output_abs.display()),
        "--force".into(),
    ];
    for pkg in extra_packages {
        args.push(format!("--package={pkg}"));
    }
    tools::run_mkosi(&args)?;
    if !tree.exists() {
        return Err(anyhow!("mkosi did not produce {}", tree.display()));
    }
    fs_err::write(&stamp_path, &stamp_key)?;
    Ok(tree.canonicalize()?)
}

/// Hash every input that defines the kernel-builder tools tree: mkosi.conf
/// (package list) plus the mkosi.sandbox tree (the apt snapshot pin lives in
/// its sources file, #96). Feeds both the rebuild stamp and the manifest's
/// tools_tree_digest so there is exactly one inventory of "the toolchain" —
/// CI's cache keys mirror it with hashFiles over the same paths plus this
/// file.
pub(super) fn tools_tree_inputs_digest() -> Result<String> {
    hash_tree_inputs(Path::new(TOOLS_TREE_CONF), Path::new(TOOLS_TREE_SANDBOX))
}

/// sha256 over sorted "path:sha256(content)\n" lines for `conf` plus every
/// file under `sandbox` — sensitive to adds, removals, renames, and edits.
fn hash_tree_inputs(conf: &Path, sandbox: &Path) -> Result<String> {
    let mut files = vec![conf.to_path_buf()];
    let mut stack = vec![sandbox.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs_err::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    for f in &files {
        hasher.update(f.as_os_str().as_encoded_bytes());
        hasher.update(b":");
        hasher.update(fetch::sha256_file(f)?.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn tools_cache_hits_with_empty_and_nonempty_extra_package_lists() {
        for packages in [vec![], vec!["qemu-system-x86".to_string()]] {
            let output = TempDir::new().unwrap();
            let tree = output.path().join("image");
            fs_err::create_dir(&tree).unwrap();
            let stamp = format!(
                "{}\n{}",
                tools_tree_inputs_digest().unwrap(),
                packages.join(",")
            );
            fs_err::write(output.path().join(".confos-tools-stamp"), stamp).unwrap();
            assert_eq!(
                ensure_tools_tree(false, &packages, output.path()).unwrap(),
                tree.canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn hash_tree_inputs_covers_conf_and_every_sandbox_file() {
        let d = TempDir::new().unwrap();
        let conf = d.path().join("mkosi.conf");
        fs_err::write(&conf, "Packages=x\n").unwrap();
        let sandbox = d.path().join("mkosi.sandbox");
        let nested = sandbox.join("etc/apt/sources.list.d");
        fs_err::create_dir_all(&nested).unwrap();
        let sources = nested.join("mkosi.sources");
        fs_err::write(&sources, "URIs: a\n").unwrap();

        let base = hash_tree_inputs(&conf, &sandbox).unwrap();
        // Deterministic across calls.
        assert_eq!(base, hash_tree_inputs(&conf, &sandbox).unwrap());
        // A nested content change moves the digest — the pin lives here (#96).
        fs_err::write(&sources, "URIs: b\n").unwrap();
        let edited = hash_tree_inputs(&conf, &sandbox).unwrap();
        assert_ne!(base, edited);
        // A new file anywhere in the tree moves it again.
        fs_err::write(sandbox.join("etc/apt/80-retries"), "x").unwrap();
        assert_ne!(edited, hash_tree_inputs(&conf, &sandbox).unwrap());
        // The conf file is covered too.
        let with_new_file = hash_tree_inputs(&conf, &sandbox).unwrap();
        fs_err::write(&conf, "Packages=y\n").unwrap();
        assert_ne!(with_new_file, hash_tree_inputs(&conf, &sandbox).unwrap());
    }
}
