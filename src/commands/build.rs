use std::path::{Path, PathBuf};

use crate::{igvm, kernel_cache, manifest, qemu, tools, BuildArgs};

pub fn run(args: &BuildArgs) -> anyhow::Result<()> {
    tracing::info!("sealing base image with dm-verity + UKI");

    let firmware = if args.skip_igvm {
        None
    } else {
        let fw = args.firmware.clone();
        if !fw.exists() {
            anyhow::bail!(
                "firmware not found: {}. Pass --skip-igvm to build without IGVM.",
                fw.display()
            );
        }
        Some(fw)
    };

    // Validate memory format before it reaches QEMU arg interpolation
    qemu::validate_memory(&args.memory)?;

    // Validate cloud-init user-data if provided
    if let Some(ref ci) = args.cloud_init {
        if !ci.exists() {
            anyhow::bail!("cloud-init user-data not found: {}", ci.display());
        }
    }

    // Validate --extra if provided
    if let Some(ref extra) = args.extra {
        if !extra.exists() {
            anyhow::bail!("--extra directory not found: {}", extra.display());
        }
        if !extra.is_dir() {
            anyhow::bail!("--extra path is not a directory: {}", extra.display());
        }
    }

    // Validate --script if provided
    if let Some(ref script) = args.script {
        if !script.exists() {
            anyhow::bail!("--script file not found: {}", script.display());
        }
        if !script.is_file() {
            anyhow::bail!("--script path is not a file: {}", script.display());
        }
    }

    // Prepare the per-build mkosi.local/ overlay. Any debris from a crashed
    // prior build is wiped so we start from a clean slate. The cleanup guard
    // is installed *before* anything writes into the overlay so that early
    // returns still trigger cleanup.
    let mkosi_local = PathBuf::from("mkosi/base/mkosi.local");
    if fs_err::exists(&mkosi_local)? {
        fs_err::remove_dir_all(&mkosi_local)?;
    }
    let mkosi_local_extra = mkosi_local.join("mkosi.extra");
    fs_err::create_dir_all(&mkosi_local_extra)?;
    let _mkosi_local_guard = MkosiLocalCleanup {
        dir: mkosi_local.clone(),
    };

    if let Some(ref extra) = args.extra {
        copy_extra(extra, &mkosi_local_extra)?;
    }

    // Check required tools — resolve mkosi's full canonical path so sudo can invoke it
    // directly (uv-installed mkosi has a symlink chain that breaks under sudo + env + PATH).
    let mkosi_bin = tools::resolve_mkosi()?;
    tracing::info!("mkosi resolved to {mkosi_bin}");

    // Prepare output directory — check for symlinks before deletion to prevent
    // remove_dir_all from following a symlink and deleting an unrelated directory.
    let dir = PathBuf::from("output").join(&args.name);
    if fs_err::exists(&dir)? {
        let meta = fs_err::symlink_metadata(&dir)?;
        if meta.is_symlink() {
            anyhow::bail!(
                "output path is a symlink (refusing to delete): {}",
                args.name.display()
            );
        }
        fs_err::remove_dir_all(&dir)?;
    }
    fs_err::create_dir_all(&dir)?;
    let output = dir.canonicalize()?;

    // Inject debug autologin if --debug (enables passwordless root on serial console)
    let autologin_dir = PathBuf::from(
        "mkosi/base/mkosi.local/mkosi.extra/etc/systemd/system/serial-getty@hvc0.service.d",
    );
    if args.console {
        println!("WARNING: --console enables passwordless root on serial console. Do not use in production.");
        inject_console_autologin(&autologin_dir)?;
    }

    // Inject cloud-init user-data into mkosi.local/mkosi.extra seed directory (measured in verity root)
    let seed_dir = PathBuf::from("mkosi/base/mkosi.local/mkosi.extra/var/lib/cloud/seed/nocloud");
    if let Some(ref ci) = args.cloud_init {
        inject_cloud_init(ci, &seed_dir)?;
    }

    // Phase 1: ensure custom kernel artifact is current
    println!("\n=== Step 1/4: Ensuring custom kernel ===");
    let kernel = kernel_cache::ensure_kernel(false, args.kernel_config_fragment.clone())?;
    println!(
        "kernel: {} (linux {})",
        kernel.vmlinuz_path.display(),
        kernel.linux_version
    );

    // Pre-stage the custom kernel into mkosi.extra so mkosi finds it during UKI assembly.
    let staged_kernel_dir = PathBuf::from("mkosi/base/mkosi.local/mkosi.extra/usr/lib/modules")
        .join(&kernel.linux_version);
    fs_err::create_dir_all(&staged_kernel_dir)?;
    fs_err::copy(&kernel.vmlinuz_path, staged_kernel_dir.join("vmlinuz"))?;

    // Step 2: Build the verity initrd via mkosi (declarative)
    println!("\n=== Step 2/4: Building verity initrd (mkosi) ===");
    let initrd_dir = PathBuf::from("mkosi/initrd");
    if !initrd_dir.exists() {
        anyhow::bail!("mkosi initrd config not found: {}", initrd_dir.display());
    }
    tools::run_command_streaming(
        "sudo",
        &[
            mkosi_bin.as_str(),
            "--directory",
            &*initrd_dir.to_string_lossy(),
            "--force",
        ],
    )?;
    let initrd_path = initrd_dir
        .join("mkosi.output/image.cpio.gz")
        .canonicalize()?;
    println!(
        "Initrd: {} ({})",
        initrd_path.display(),
        human_size(&initrd_path)?
    );

    // Step 3: Run mkosi — builds disk with verity, UKI with root hash + our initrd + modules
    println!("\n=== Step 3/4: Building image with mkosi (verity + UKI) ===");
    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    let mut mkosi_args: Vec<String> = vec![
        mkosi_bin.clone(),
        "--directory".to_string(),
        mkosi_dir.to_string_lossy().into_owned(),
        "--force".to_string(),
        "--initrd".to_string(),
        initrd_path.to_string_lossy().into_owned(),
    ];
    for pkg in &args.package {
        mkosi_args.push(format!("--package={pkg}"));
    }
    if let Some(ref script) = args.script {
        // mkosi resolves --postinst-script relative to --directory, so anchor
        // the user's path with canonicalize before handing it off. Enable
        // network access so the script can fetch resources from the internet.
        let canonical = script.canonicalize()?;
        mkosi_args.push(format!("--postinst-script={}", canonical.display()));
        mkosi_args.push("--with-network=yes".to_string());
    }
    tools::run_command_streaming("sudo", &mkosi_args)?;

    let mkosi_output = mkosi_dir.join("mkosi.output");
    // Find the split artifacts mkosi produced
    let uki_path = mkosi_output.join("image.efi");
    if !uki_path.exists() {
        anyhow::bail!("UKI .efi not found in mkosi output. Check mkosi build logs.");
    }
    let base_image = mkosi_output.join("image.raw");
    if !base_image.exists() {
        anyhow::bail!("image.raw not found in mkosi output. Check mkosi build logs.");
    }

    // Copy UKI to output
    let output_uki = output.join("uki.efi");
    tools::sudo_mv(&uki_path, &output_uki)?;

    // Read roothash (produced by mkosi SplitArtifacts=roothash)
    let roothash_path = mkosi_output.join("image.roothash");
    if !roothash_path.exists() {
        anyhow::bail!("image.roothash not found — check mkosi.conf has SplitArtifacts=roothash");
    }
    tools::sudo_chmod_readable(&roothash_path)?;
    let roothash = fs_err::read_to_string(&roothash_path)?
        .trim()
        .to_lowercase();
    let valid_lengths = [64, 96, 128]; // SHA-256, SHA-384, SHA-512
    if !valid_lengths.contains(&roothash.len()) || !roothash.chars().all(|c| c.is_ascii_hexdigit())
    {
        anyhow::bail!(
            "invalid roothash from mkosi: {roothash:?} (expected 64/96/128 hex chars, got {})",
            roothash.len()
        );
    }
    fs_err::write(output.join("roothash"), &roothash)?;
    println!("Root hash: {roothash}");
    println!(
        "UKI: {} ({})",
        output_uki.display(),
        human_size(&output_uki)?
    );

    // Step 3: Build IGVM variants (optional). Emits one `guest-smp{N}.igvm`
    // per value in `args.smp` (default [2, 4, 8, 16] — the standard
    // powers-of-two), each as its own entry in manifest.variants[]. The
    // firmware + UKI bytes are read once and reused; the per-variant cost
    // is just the measurement pass, so building the default set adds
    // sub-second to the overall build.
    let igvm_variants: Vec<manifest::IgvmVariant> = if args.skip_igvm {
        println!("\n=== Step 4/4: Skipping IGVM (--skip-igvm) ===");
        Vec::new()
    } else {
        println!(
            "\n=== Step 4/4: Building IGVM variants (smp = {:?}) ===",
            args.smp
        );

        // firmware is guaranteed Some when skip_igvm is false (validated at top)
        let fw_path = firmware
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("firmware path required for IGVM build"))?;
        let fw_bytes = fs_err::read(fw_path)?;
        let uki_bytes = fs_err::read(&output_uki)?;

        // Sort + dedup so the on-disk manifest has a canonical ordering
        // regardless of how the operator listed --smp.
        let mut smps = args.smp.clone();
        smps.sort_unstable();
        smps.dedup();
        if smps.is_empty() {
            anyhow::bail!("--smp must list at least one vCPU count");
        }

        let mut out = Vec::with_capacity(smps.len());
        for smp in smps {
            if smp == 0 {
                anyhow::bail!("SMP count must be >= 1, got 0");
            }
            print!("  smp={smp} ... ");
            let result = igvm::invoke::build_snp(&fw_bytes, &uki_bytes, smp)?;

            let igvm_name = format!("guest-smp{smp}.igvm");
            let igvm_path = output.join(&igvm_name);
            fs_err::write(&igvm_path, &result.igvm_bytes)?;

            let digest = hex::encode(result.measurement.launch_digest);
            println!(
                "{} ({}, digest: {}...{})",
                igvm_name,
                human_size(&igvm_path)?,
                &digest[..8],
                &digest[digest.len() - 8..],
            );

            let igvm_sha256 = manifest::sha256_file(&igvm_path)?;
            out.push(manifest::IgvmVariant {
                smp,
                igvm: manifest::FileEntry {
                    path: igvm_name,
                    sha256: igvm_sha256,
                },
                measurement: manifest::Measurement {
                    snp_launch_digest: digest,
                    algorithm: "sha384".to_string(),
                    page_count: result.measurement.page_count,
                    vmsa_count: result.measurement.vmsa_count,
                },
            });
        }
        out
    };

    // Copy firmware into output so the directory is self-contained for publish/run
    if let Some(ref fw) = firmware {
        let output_fw = output.join("OVMF.fd");
        fs_err::copy(fw, &output_fw)?;
        println!("Firmware: {}", output_fw.display());
    }

    // move raw disk image to output
    let disk_path = output.join("disk.raw");
    let base_abs = base_image.canonicalize()?;
    tools::sudo_mv(&base_abs, &disk_path)?;

    println!("\n=== Calculating checksums ===");
    // Read the disk checksum from the mkosi output
    let mkosi_checksums = fs_err::read(mkosi_output.join("image.SHA256SUMS"))?;
    let disk_checksum = String::try_from(mkosi_checksums)?
        .split("\n")
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad checksum file"))?
        .split(" ")
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad checksum file"))?
        .to_owned();
    println!("disk.raw {}", disk_checksum);

    // calculate the other checksums
    let initrd_hash = manifest::sha256_file(&initrd_path)?;
    println!("initrd   {}", initrd_hash);
    for v in &igvm_variants {
        println!("igvm     {} ({})", v.igvm.sha256, v.igvm.path);
    }
    let uki_hash = manifest::sha256_file(&output_uki)?;
    println!("uki      {}", uki_hash);

    println!("\n=== Writing manifest.json ===");
    // Write manifest
    let build_manifest = manifest::BuildManifest {
        version: manifest::MANIFEST_VERSION,
        build: manifest::BuildConfig {
            timestamp: chrono_now(),
            memory: args.memory.clone(),
            format: "raw".to_string(),
            platform: if args.skip_igvm {
                "generic".to_string()
            } else {
                "snp".to_string()
            },
        },
        inputs: manifest::ManifestInputs {
            kernel: Some(manifest::KernelInputs {
                linux_version: kernel.linux_version.clone(),
                vmlinuz_sha256: kernel.manifest.outputs.vmlinuz_sha256.clone(),
                required_config_sha256: kernel.manifest.inputs.required_config_sha256.clone(),
                hardening_config_sha256: kernel.manifest.inputs.hardening_config_sha256.clone(),
                kernel_extra_config_sha256: kernel
                    .manifest
                    .inputs
                    .kernel_extra_config_sha256
                    .clone(),
                snapshot_config_sha256: kernel.manifest.inputs.snapshot_config_sha256.clone(),
            }),
            initrd: manifest::FileEntry {
                path: manifest::basename_of(&initrd_path),
                sha256: initrd_hash,
            },
            firmware: firmware
                .as_ref()
                .map(|fw| -> anyhow::Result<manifest::FileEntry> {
                    Ok(manifest::FileEntry {
                        path: manifest::basename_of(fw),
                        sha256: manifest::sha256_file(fw)?,
                    })
                })
                .transpose()?,
            base_image: manifest::FileEntry {
                path: manifest::basename_of(&base_abs),
                sha256: disk_checksum.to_owned(),
            },
        },
        outputs: manifest::ManifestOutputs {
            disk_image: manifest::FileEntry {
                path: manifest::basename_of(&disk_path),
                sha256: disk_checksum,
            },
            uki: manifest::FileEntry {
                path: manifest::basename_of(&output_uki),
                sha256: uki_hash,
            },
        },
        variants: igvm_variants,
    };
    let manifest_path = output.join("manifest.json");
    manifest::write_manifest(&build_manifest, &manifest_path)?;

    println!("\n===============================");
    println!("  Build complete!");
    println!("  Output:     {}", output.display());
    for v in &build_manifest.variants {
        println!("  IGVM:       {} (smp={})", v.igvm.path, v.smp);
    }
    println!("  Disk:       {}", disk_path.display());
    println!("  Manifest:   {}", manifest_path.display());
    println!("  Root hash:  {roothash}");
    for v in &build_manifest.variants {
        println!(
            "  Launch digest (smp={}): {}",
            v.smp, v.measurement.snp_launch_digest
        );
    }
    if args.cloud_init.is_some() {
        println!("  Cloud-init: measured in verity root");
    }
    if args.console {
        println!("  Debug:      autologin enabled (NOT for production)");
    }
    println!("===============================");

    Ok(())
}

/// Inject a systemd drop-in that enables passwordless root autologin on ttyS0.
/// Only used with --debug; changes the image measurement.
fn inject_console_autologin(dir: &Path) -> anyhow::Result<()> {
    fs_err::create_dir_all(dir)?;
    fs_err::write(
        dir.join("autologin.conf"),
        "[Service]\nExecStart=\nExecStart=-/sbin/agetty -o '-p -f -- \\\\u' --noclear --autologin root --keep-baud 115200,57600,38400,9600 %I $TERM\n",
    )?;
    Ok(())
}

/// Inject cloud-init user-data into the mkosi.local/mkosi.extra seed directory.
/// The NoCloud datasource picks up user-data from /var/lib/cloud/seed/nocloud/.
fn inject_cloud_init(user_data: &Path, seed_dir: &Path) -> anyhow::Result<()> {
    fs_err::create_dir_all(seed_dir)?;

    // Copy user-data
    fs_err::copy(user_data, seed_dir.join("user-data"))?;

    // Create minimal meta-data
    fs_err::write(
        seed_dir.join("meta-data"),
        "instance-id: steep-sealed\nlocal-hostname: steep\n",
    )?;

    println!("Cloud-init: config measured in image, will run at boot");

    Ok(())
}

/// RAII guard that removes the entire per-build `mkosi.local/` overlay after
/// the mkosi run, including when an error path drops the guard early. All
/// per-build file injections (extra, kernel, console, cloud-init) live under
/// this directory, so a single cleanup covers them all.
struct MkosiLocalCleanup {
    dir: PathBuf,
}

impl Drop for MkosiLocalCleanup {
    fn drop(&mut self) {
        let _ = tools::force_remove_dir_all(&self.dir);
    }
}

/// Recursively copy the contents of `src` into `dst`.
///
/// - `src` must be an existing directory (caller validates).
/// - `dst` is created if missing.
/// - Files preserve their unix mode bits.
/// - Symlinks are copied as symlinks (target path verbatim, not dereferenced).
fn copy_extra(src: &Path, dst: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs_err::create_dir_all(dst)?;
    for entry in fs_err::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            let target = fs_err::read_link(&from)?;
            // If the destination already exists, remove it so symlink() doesn't fail.
            if fs_err::symlink_metadata(&to).is_ok() {
                let _ = fs_err::remove_file(&to);
            }
            std::os::unix::fs::symlink(&target, &to)?;
        } else if ft.is_dir() {
            copy_extra(&from, &to)?;
        } else {
            fs_err::copy(&from, &to)?;
            let mode = fs_err::metadata(&from)?.permissions().mode();
            fs_err::set_permissions(&to, std::fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

fn human_size(path: &Path) -> anyhow::Result<String> {
    let bytes = fs_err::metadata(path)?.len();
    Ok(humansize::format_size(bytes, humansize::BINARY))
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn copy_extra_copies_files_at_root() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        fs_err::write(src.path().join("a.txt"), b"hello").unwrap();

        copy_extra(src.path(), dst.path()).unwrap();

        let copied = fs_err::read(dst.path().join("a.txt")).unwrap();
        assert_eq!(copied, b"hello");
    }

    #[test]
    fn copy_extra_copies_nested_directories() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        fs_err::create_dir_all(src.path().join("etc/foo")).unwrap();
        fs_err::write(src.path().join("etc/foo/bar.conf"), b"x=1").unwrap();

        copy_extra(src.path(), dst.path()).unwrap();

        assert_eq!(
            fs_err::read(dst.path().join("etc/foo/bar.conf")).unwrap(),
            b"x=1"
        );
    }

    #[test]
    fn copy_extra_preserves_file_modes() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        let path = src.path().join("script");
        fs_err::write(&path, b"#!/bin/sh\n").unwrap();
        fs_err::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        copy_extra(src.path(), dst.path()).unwrap();

        let mode = fs_err::metadata(dst.path().join("script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn copy_extra_preserves_symlinks() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        fs_err::write(src.path().join("target"), b"t").unwrap();
        std::os::unix::fs::symlink("target", src.path().join("link")).unwrap();

        copy_extra(src.path(), dst.path()).unwrap();

        let link_meta = fs_err::symlink_metadata(dst.path().join("link")).unwrap();
        assert!(link_meta.file_type().is_symlink());
        let target = fs_err::read_link(dst.path().join("link")).unwrap();
        assert_eq!(target, std::path::PathBuf::from("target"));
    }

    #[test]
    fn copy_extra_empty_source_is_ok() {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        copy_extra(src.path(), dst.path()).unwrap();
        // dst should exist and be empty
        assert!(dst.path().exists());
        assert_eq!(fs_err::read_dir(dst.path()).unwrap().count(), 0);
    }

    #[test]
    fn copy_extra_creates_destination_if_missing() {
        let src = TempDir::new().unwrap();
        let dst_parent = TempDir::new().unwrap();
        let dst = dst_parent.path().join("does/not/exist/yet");
        fs_err::write(src.path().join("f"), b"x").unwrap();

        copy_extra(src.path(), &dst).unwrap();

        assert_eq!(fs_err::read(dst.join("f")).unwrap(), b"x");
    }

    #[test]
    fn copy_extra_fails_on_nonexistent_source() {
        let parent = TempDir::new().unwrap();
        let src = parent.path().join("nonexistent-child");
        let dst = TempDir::new().unwrap();
        let result = copy_extra(&src, dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn copy_extra_fails_on_file_source() {
        let parent = TempDir::new().unwrap();
        let src = parent.path().join("a-file");
        fs_err::write(&src, b"x").unwrap();
        let dst = TempDir::new().unwrap();
        let result = copy_extra(&src, dst.path());
        assert!(result.is_err());
    }

    #[test]
    fn mkosi_local_cleanup_removes_directory_on_drop() {
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("mkosi.local");
        fs_err::create_dir_all(dir.join("mkosi.extra/etc")).unwrap();
        fs_err::write(dir.join("mkosi.extra/etc/file"), b"x").unwrap();

        {
            let _guard = MkosiLocalCleanup { dir: dir.clone() };
            assert!(dir.exists());
        }
        assert!(!dir.exists());
    }

    #[test]
    fn mkosi_local_cleanup_swallows_missing_directory() {
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("never-existed");
        drop(MkosiLocalCleanup { dir });
        // No panic == pass.
    }
}
