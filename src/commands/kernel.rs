use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};

use crate::kernel::{aml, compile, config, fetch, manifest as km, version::KernelVersion};
use crate::tools;
use crate::{KernelArgs, KernelSourceArgs};

const REQUIRED_FRAGMENT: &str = "kernel/required.config";
/// Committed, public RANDSTRUCT/latent_entropy seed — pins per-build struct
/// randomization so vmlinuz reproduces (#85). Its sha256 joins the fingerprint.
const RANDSTRUCT_SEED: &str = "kernel/randstruct.seed";
/// Where `--module-signing-cert` is staged inside the kernel tree. Fragments
/// that enable module signing point CONFIG_SYSTEM_TRUSTED_KEYS at this path,
/// so it is part of the interface — changing it breaks every consumer's
/// fragment. See docs/module-signing.md.
const STAGED_SIGNING_CERT: &str = "confos-module-signing.crt";
/// Default module signing certificate, used when `--module-signing-cert` is
/// omitted. Public by construction — it is built into the system keyring and
/// therefore shipped inside every image — so committing it is what lets a
/// fresh clone build a reproducible image with no setup. A consumer that
/// wants its own signing key passes the flag instead.
const DEFAULT_SIGNING_CERT: &str = "kernel/module-signing.crt";
const HARDENING_FRAGMENT: &str = "kernel/hardening.config";
/// Confidential VM overrides. Merged after `hardening.config` so the last
/// fragment wins. Resolved trusted-AML requirements are additionally enforced
/// after consumer overrides, so a later fragment cannot weaken the boundary.
const CVM_FRAGMENT: &str = "kernel/confidential.config";
/// Bare-baseline snapshot lockfile (committed). Fragment builds write
/// `config-x86_64-<stem>.snapshot` beside their fragment, so lineages don't
/// clobber it and consumers can commit theirs in their own repo (#66).
const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const VERSION_PATH: &str = "kernel/version";
const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";
const TOOLS_TREE_CONF: &str = "mkosi/kernel-builder/mkosi.conf";
const TOOLS_TREE_SANDBOX: &str = "mkosi/kernel-builder/mkosi.sandbox";
const TOOLS_TREE_OUTPUT: &str = "mkosi/kernel-builder/mkosi.output";
/// The ACPI harness's tools tree: the production packages plus QEMU, Python
/// and a static libc. It lives beside the production tree so neither
/// invalidates the other, and the harness packages never enter the measured
/// toolchain.
const HARNESS_TOOLS_TREE_OUTPUT: &str = "mkosi/kernel-builder/mkosi.output-acpi";
const HARNESS_TOOLS_PACKAGES: [&str; 3] = ["libc6-dev", "python3", "qemu-system-x86"];
const ACPI_HARNESS_DIR: &str = "tests/acpi";

pub fn run(args: &KernelArgs) -> Result<()> {
    let version = pinned_version()?;
    tracing::info!(linux_version = %version.linux_version, "building hardened kernel");

    // Optional caller-supplied config fragment merged after required +
    // hardening. No flag = confos's bare required + hardening baseline.
    let fragment = args.kernel_inputs.kernel_config_fragment.as_deref();
    // Caller's certificate, else the committed default (see
    // DEFAULT_SIGNING_CERT). Always resolves to something, so every build
    // that enables module signing has a trust anchor without the caller
    // arranging one.
    let default_cert = PathBuf::from(DEFAULT_SIGNING_CERT);
    let signing_cert = args
        .kernel_inputs
        .module_signing_cert
        .as_deref()
        .unwrap_or(&default_cert);
    let snapshot = snapshot_path(fragment)?;
    let snapshot = snapshot.as_path();

    // Consumer inputs get their own messages before the fingerprint reads them.
    if let Some(f) = fragment {
        if !f.exists() {
            return Err(anyhow!(
                "--kernel-config-fragment path not found: {}",
                f.display()
            ));
        }
    }
    verify_signing_cert(signing_cert)?;

    fs_err::create_dir_all(&args.output)?;
    let out_dir = args.output.canonicalize()?;
    let cache_dir = out_dir.join("cache");
    let build_dir = out_dir.join("build");
    let log_path = out_dir.join("build.log");
    let vmlinuz_path = out_dir.join("vmlinuz");
    let manifest_path = out_dir.join("manifest.json");

    // Every input is hashed once here, before anything is staged, and once
    // more after the compile: an edit during the build fails it instead of
    // labelling the old bytes with the new hashes.
    let staged = compute_fingerprint(&version, fragment, signing_cert, snapshot)?;

    // Cache short-circuit: skip the entire build if all inputs match and the
    // existing vmlinuz still hashes to what the manifest claims. --force
    // bypasses this.
    if !args.force && manifest_path.exists() && vmlinuz_path.exists() {
        if let Ok(cached) = km::read(&manifest_path) {
            if cached.inputs == staged {
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

    // Phase 0a: ensure tools tree
    println!("\n=== Step 0a: Ensuring kernel-builder tools tree (mkosi) ===");
    let tools_tree = ensure_tools_tree(
        args.force,
        &args.kernel_inputs.kernel_builder_package,
        Path::new(TOOLS_TREE_OUTPUT),
    )?;

    // Phase 0b + 0c: fetch, extract, stage the trusted AML inputs, configure
    println!("\n=== Step 0b: Fetching + extracting kernel ===");
    // Discard previous versions' source and objects before a fresh build.
    tools::force_remove_dir_all(&build_dir)?;
    let kernel_src = extract_pinned_source(&version, &tools_tree, &cache_dir, &build_dir)?;
    println!("\n=== Step 0c: Configuring kernel ===");

    // Pin the RANDSTRUCT seed: rewrite gen-randstruct-seed.sh to emit our
    // committed seed instead of reading /dev/urandom. The Makefile rule is
    // FORCE + if_changed, so staging the .seed file alone would be
    // regenerated — the script is the deterministic point.
    let seed = fs_err::read_to_string(Path::new(RANDSTRUCT_SEED))?;
    let seed = seed.trim();
    if seed.len() != 64 || !seed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!("{RANDSTRUCT_SEED} must be 64 hex chars (32 bytes)"));
    }
    let gen = kernel_src.join("scripts/gen-randstruct-seed.sh");
    fs_err::write(
        &gen,
        format!(
            "#!/bin/sh\n\
             # Replaced by confos: fixed seed for reproducible builds (#85).\n\
             SEED={seed}\n\
             echo \"$SEED\" > \"$1\"\n\
             HASH=$(echo -n \"$SEED\" | sha256sum | cut -d' ' -f1)\n\
             echo \"#define RANDSTRUCT_HASHED_SEED \\\"$HASH\\\"\" > \"$2\"\n"
        ),
    )?;
    let mut perms = fs_err::metadata(&gen)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs_err::set_permissions(&gen, perms)?;

    // Pin BTF encoding to one thread: Makefile.btf passes pahole -j$(JOBS),
    // and parallel BTF dedup is order-nondeterministic — a real hazard, kept
    // as hardening. (The #85 nondeterminism itself proved to be the
    // MODULE_SIG_KEY GENKEY, not BTF: diffoscope showed no .BTF delta.)
    let btf_mk = kernel_src.join("scripts/Makefile.btf");
    let mk = fs_err::read_to_string(&btf_mk)?;
    let pinned = mk.replace("-j$(JOBS)", "-j1");
    if pinned == mk {
        return Err(anyhow!(
            "scripts/Makefile.btf has no -j$(JOBS) to pin — kernel bump changed BTF flags; re-check #85 determinism"
        ));
    }
    fs_err::write(&btf_mk, pinned)?;
    // Stage the caller's signing certificate where CONFIG_SYSTEM_TRUSTED_KEYS
    // expects it (STAGED_SIGNING_CERT). The certificate is a consumer input,
    // like the config fragment: whoever owns the image owns the trust anchor
    // for the modules it loads, and this repo ships none.
    let certs_dir = kernel_src.join("certs");
    fs_err::create_dir_all(&certs_dir)?;
    // kernel_src is a freshly extracted tarball (wiped above), so a plain copy
    // is enough — no staging guard as for --profile-dir.
    fs_err::copy(signing_cert, certs_dir.join(STAGED_SIGNING_CERT))?;

    config::run_configure_phase(
        &tools_tree,
        &kernel_src,
        Path::new(REQUIRED_FRAGMENT),
        Path::new(HARDENING_FRAGMENT),
        Path::new(CVM_FRAGMENT),
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
    compile::run(&tools_tree, &kernel_src, &vmlinuz_path, seed, &log_path)?;

    // Phase 0e: finalize manifest
    println!("\n=== Step 0e: Writing manifest ===");
    let inputs = compute_fingerprint(&version, fragment, signing_cert, snapshot)?;
    verify_inputs_unchanged(&staged, &inputs)?;
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

/// `confos kernel-source`: the same pinned, checksum-verified, patched tree
/// the build compiles, with the trusted DSDT header generated in the pinned
/// tools tree, but left as source at `<output>/linux-<version>`. The ACPI
/// harness (tests/acpi) boots kernels built from it, so what it tests is what
/// the builder ships, prepared by one code path; `--acpi-harness` runs it in
/// the same tools tree afterwards.
pub fn prepare_source(args: &KernelSourceArgs) -> Result<()> {
    if !args.acpi_harness && !args.harness_args.is_empty() {
        bail!("arguments after `--` go to the harness; pass --acpi-harness");
    }
    let version = pinned_version()?;
    let packages: Vec<String> = HARNESS_TOOLS_PACKAGES.map(String::from).to_vec();
    let tools_tree = ensure_tools_tree(false, &packages, Path::new(HARNESS_TOOLS_TREE_OUTPUT))?;
    fs_err::create_dir_all(&args.output)?;
    let out_dir = args.output.canonicalize()?;
    let kernel_src =
        extract_pinned_source(&version, &tools_tree, &out_dir.join("cache"), &out_dir)?;
    println!("kernel source: {}", kernel_src.display());
    if args.acpi_harness {
        run_acpi_harness(&tools_tree, &out_dir, &version, &args.harness_args)?;
    }
    Ok(())
}

/// Boot the harness's test kernels inside the tools tree: the output
/// directory (tree, cache, results) is `/work` and tests/acpi is bound
/// read-only at `/harness`.
fn run_acpi_harness(
    tools_tree: &Path,
    out_dir: &Path,
    version: &KernelVersion,
    extra_args: &[String],
) -> Result<()> {
    let harness = Path::new(ACPI_HARNESS_DIR).canonicalize()?;
    let mut argv: Vec<OsString> = [
        "python3",
        "/harness/run.py",
        "--source",
        &format!("/work/linux-{}", version.linux_version),
        "--output",
        "/work/results",
    ]
    .map(OsString::from)
    .to_vec();
    argv.extend(extra_args.iter().map(OsString::from));
    config::nspawn_exec(
        tools_tree,
        &[
            config::Bind::rw(out_dir, "/work"),
            config::Bind::ro(&harness, "/harness"),
        ],
        &[],
        &argv,
    )
}

/// The committed kernel pin, admitted only if the trusted-AML patch was
/// audited against it. Every entry point reads the pin through here.
fn pinned_version() -> Result<KernelVersion> {
    let version = KernelVersion::read(Path::new(VERSION_PATH))?;
    aml::verify_version(&version.linux_version)?;
    Ok(version)
}

/// Fetch the pinned tarball into `cache_dir`, extract it fresh under `dest`,
/// and patch the tree and compile the trusted table inside `tools_tree`.
/// Returns the prepared tree.
fn extract_pinned_source(
    version: &KernelVersion,
    tools_tree: &Path,
    cache_dir: &Path,
    dest: &Path,
) -> Result<PathBuf> {
    let tarball = fetch::fetch(&version.linux_version, &version.tarball_sha256, cache_dir)?;
    let kernel_src = dest.join(format!("linux-{}", version.linux_version));
    // The tools tree writes into this tree as root via nspawn, so a previous
    // run can leave root-owned files here. force_remove_dir_all falls back
    // to `sudo rm -rf` on EPERM so re-runs always succeed.
    tools::force_remove_dir_all(&kernel_src)?;
    fs_err::create_dir_all(dest)?;
    extract_tarball(&tarball, dest)?;
    if !kernel_src.exists() {
        return Err(anyhow!(
            "expected extracted dir {} not found",
            kernel_src.display()
        ));
    }
    aml::stage(tools_tree, &kernel_src)?;
    Ok(kernel_src)
}

fn verify_signing_cert(signing_cert: &Path) -> Result<()> {
    if !signing_cert.is_file() {
        return Err(anyhow!(
            "module signing certificate not found: {} — pass --module-signing-cert, \
             or generate the default per docs/module-signing.md",
            signing_cert.display()
        ));
    }
    // A placeholder (or any non-PEM) would otherwise fail deep in the build at
    // the .incbin in certs/system_certificates.S; say so here instead.
    if !fs_err::read_to_string(signing_cert)
        .unwrap_or_default()
        .contains("-----BEGIN CERTIFICATE-----")
    {
        return Err(anyhow!(
            "{} is not a PEM certificate (docs/module-signing.md)",
            signing_cert.display()
        ));
    }
    Ok(())
}

/// The snapshot lockfile is the one input the build itself rewrites; every
/// other field must still hash to what was staged, or the manifest would
/// describe bytes that were not built.
fn verify_inputs_unchanged(staged: &km::Fingerprint, finished: &km::Fingerprint) -> Result<()> {
    let expected = km::Fingerprint {
        snapshot_config_sha256: finished.snapshot_config_sha256.clone(),
        ..staged.clone()
    };
    if *finished != expected {
        bail!("kernel inputs changed during the build; rerun `confos kernel`");
    }
    Ok(())
}

/// Build the kernel-builder tools tree under `output` if needed, return
/// its image path.
///
/// Skips the (slow, sudo-requiring) `mkosi --force` rebuild when a previous
/// build's stamp file matches the current `mkosi.conf` hash. `force` bypasses
/// the skip. The stamp lives under `output`, which `mkosi --force`
/// wipes — so a successful rebuild always lands a fresh stamp, and a failed
/// rebuild leaves no stamp behind to fool a later cache check.
fn ensure_tools_tree(force: bool, extra_packages: &[String], output: &Path) -> Result<PathBuf> {
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
            if stamped.trim() == stamp_key {
                println!("kernel-builder tools tree cache HIT (mkosi.conf + packages unchanged)");
                return Ok(tree.canonicalize()?);
            }
        }
    }

    // Wipe stale stamp before rebuild so a half-failed `mkosi --force` can't
    // be picked up as a cache hit on the next call.
    let _ = fs_err::remove_file(&stamp_path);

    // mkosi chdirs into --directory before reading paths, so hand it the
    // output directory absolute.
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

/// Snapshot lockfile path for this lineage: the committed bare one without a
/// fragment, `config-x86_64-<stem>.snapshot` beside the fragment with one.
fn snapshot_path(fragment: Option<&Path>) -> Result<PathBuf> {
    let Some(f) = fragment else {
        return Ok(PathBuf::from(SNAPSHOT_PATH));
    };
    let stem = f
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && !s.starts_with('.'))
        .ok_or_else(|| {
            anyhow!(
                "--kernel-config-fragment has no usable file stem: {}",
                f.display()
            )
        })?;
    let dir = f.parent().unwrap_or_else(|| Path::new(""));
    Ok(dir.join(format!("config-x86_64-{stem}.snapshot")))
}

/// Compute the fingerprint over all inputs that determine kernel build output.
///
/// `fragment` is the caller-supplied `--kernel-config-fragment` (None when
/// building confos's bare baseline). `snapshot` is the committed snapshot
/// lockfile; hashing it into the fingerprint means a deleted or hand-edited
/// snapshot invalidates the cache and forces a rebuild that regenerates it.
pub fn compute_fingerprint(
    version: &KernelVersion,
    fragment: Option<&Path>,
    signing_cert: &Path,
    snapshot: &Path,
) -> Result<km::Fingerprint> {
    Ok(km::Fingerprint {
        linux_version: version.linux_version.clone(),
        tarball_sha256: version.tarball_sha256.clone(),
        required_config_sha256: fetch::sha256_file(Path::new(REQUIRED_FRAGMENT))?,
        hardening_config_sha256: fetch::sha256_file(Path::new(HARDENING_FRAGMENT))?,
        confidential_config_sha256: fetch::sha256_file(Path::new(CVM_FRAGMENT))?,
        module_signing_cert_sha256: fetch::sha256_file(signing_cert)?,
        randstruct_seed_sha256: fetch::sha256_file(Path::new(RANDSTRUCT_SEED))?,
        trusted_dsdt_sha256: fetch::sha256_file(Path::new(aml::DSDT_SOURCE))?,
        trusted_aml_patch_sha256: fetch::sha256_file(Path::new(aml::PATCH))?,
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
        tools_tree_digest: tools_tree_inputs_digest()?,
    })
}

/// Hash every input that defines the kernel-builder tools tree: mkosi.conf
/// (package list) plus the mkosi.sandbox tree (the apt snapshot pin lives in
/// its sources file, #96). Feeds both the rebuild stamp and the manifest's
/// tools_tree_digest so there is exactly one inventory of "the toolchain" —
/// CI's cache keys mirror it with hashFiles over the same paths.
fn tools_tree_inputs_digest() -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

    #[test]
    fn mid_build_input_edits_fail_except_the_regenerated_snapshot() {
        let staged = km::Fingerprint {
            linux_version: "6.18.49".into(),
            tarball_sha256: "a".repeat(64),
            required_config_sha256: "b".repeat(64),
            hardening_config_sha256: "c".repeat(64),
            confidential_config_sha256: "d".repeat(64),
            kernel_extra_config_sha256: String::new(),
            snapshot_config_sha256: "e".repeat(64),
            module_signing_cert_sha256: "f".repeat(64),
            randstruct_seed_sha256: "1".repeat(64),
            trusted_dsdt_sha256: "2".repeat(64),
            trusted_aml_patch_sha256: "3".repeat(64),
            tools_tree_digest: "4".repeat(64),
        };
        verify_inputs_unchanged(&staged, &staged).unwrap();
        let mut regenerated = staged.clone();
        regenerated.snapshot_config_sha256 = "9".repeat(64);
        verify_inputs_unchanged(&staged, &regenerated).unwrap();
        let mut edited = staged.clone();
        edited.trusted_dsdt_sha256 = "9".repeat(64);
        assert!(verify_inputs_unchanged(&staged, &edited).is_err());
        let mut edited = staged.clone();
        edited.hardening_config_sha256 = "9".repeat(64);
        assert!(verify_inputs_unchanged(&staged, &edited).is_err());
    }

    #[test]
    fn snapshot_path_is_per_lineage() {
        assert_eq!(
            snapshot_path(None).unwrap(),
            PathBuf::from("kernel/config-x86_64.snapshot")
        );
        // Beside the fragment: a consumer's lineage lockfile lands in the
        // consumer's tree, committable there.
        assert_eq!(
            snapshot_path(Some(Path::new("/repo/kernel/c8s.config"))).unwrap(),
            PathBuf::from("/repo/kernel/config-x86_64-c8s.snapshot")
        );
        assert_eq!(
            snapshot_path(Some(Path::new("c8s-dev.config"))).unwrap(),
            PathBuf::from("config-x86_64-c8s-dev.snapshot")
        );
        // Lineages must never share a lockfile: fragment vs bare, and
        // same-stem fragments in different directories.
        assert_ne!(
            snapshot_path(Some(Path::new("kernel/gpu.config"))).unwrap(),
            snapshot_path(None).unwrap()
        );
        assert_ne!(
            snapshot_path(Some(Path::new("/a/gpu.config"))).unwrap(),
            snapshot_path(Some(Path::new("/b/gpu.config"))).unwrap()
        );
        assert!(snapshot_path(Some(Path::new("/"))).is_err());
        assert!(snapshot_path(Some(Path::new("/repo/.config"))).is_err());
    }
}
