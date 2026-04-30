use std::path::{Path, PathBuf};

use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
};
use crate::{tools, BuildArgs};

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
    crate::qemu::validate_memory(&args.memory)?;

    // Validate cloud-init user-data if provided
    if let Some(ref ci) = args.cloud_init {
        if !ci.exists() {
            anyhow::bail!("cloud-init user-data not found: {}", ci.display());
        }
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
    let autologin_dir =
        PathBuf::from("mkosi/base/mkosi.extra/etc/systemd/system/serial-getty@hvc0.service.d");
    let _console_guard = if args.console {
        println!("WARNING: --console enables passwordless root on serial console. Do not use in production.");
        inject_console_autologin(&autologin_dir)?;
        Some(ConsoleCleanup { dir: autologin_dir })
    } else {
        None
    };

    // Inject cloud-init user-data into mkosi.extra seed directory (measured in verity root)
    let seed_dir = PathBuf::from("mkosi/base/mkosi.extra/var/lib/cloud/seed/nocloud");
    let _cloud_init_guard = if let Some(ref ci) = args.cloud_init {
        inject_cloud_init(ci, &seed_dir)?;
        Some(CloudInitCleanup { seed_dir })
    } else {
        None
    };

    // Phase 1: ensure custom kernel artifact is current
    println!("\n=== Step 1/4: Ensuring custom kernel ===");
    let kernel = crate::kernel_cache::ensure_kernel(false)?;
    println!(
        "kernel: {} (linux {})",
        kernel.vmlinuz_path.display(),
        kernel.linux_version
    );

    // Pre-stage the custom kernel into mkosi.extra so mkosi finds it during UKI assembly.
    let staged_kernel_dir =
        PathBuf::from("mkosi/base/mkosi.extra/usr/lib/modules").join(&kernel.linux_version);
    fs_err::create_dir_all(&staged_kernel_dir)?;
    let staged_kernel = staged_kernel_dir.join("vmlinuz");
    fs_err::copy(&kernel.vmlinuz_path, &staged_kernel)?;
    let _kernel_stage_guard = KernelStageCleanup {
        staged: staged_kernel,
    };

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

    // Step 3: Run mkosi — builds disk with verity, UKI with roothash + our initrd + modules
    println!("\n=== Step 3/4: Building image with mkosi (verity + UKI) ===");
    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    tools::run_command_streaming(
        "sudo",
        &[
            mkosi_bin.as_str(),
            "--directory",
            &*mkosi_dir.to_string_lossy(),
            "--force",
            "--initrd",
            &*initrd_path.to_string_lossy(),
        ],
    )?;

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

    // Step 3: Build IGVM (optional)
    let igvm_path = output.join("guest.igvm");
    let measurement = if args.skip_igvm {
        println!("\n=== Step 4/4: Skipping IGVM (--skip-igvm) ===");
        None
    } else {
        println!("\n=== Step 4/4: Building IGVM ===");

        // firmware is guaranteed Some when skip_igvm is false (validated at top)
        let fw_path = firmware
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("firmware path required for IGVM build"))?;
        let fw_bytes = fs_err::read(fw_path)?;
        let uki_bytes = fs_err::read(&output_uki)?;

        let result = crate::igvm::invoke::build_snp(&fw_bytes, &uki_bytes, args.smp)?;

        fs_err::write(&igvm_path, &result.igvm_bytes)?;
        println!(
            "IGVM: {} ({})",
            igvm_path.display(),
            human_size(&igvm_path)?
        );

        Some(Measurement {
            snp_launch_digest: hex::encode(result.measurement.launch_digest),
            algorithm: "sha384".to_string(),
            page_count: result.measurement.page_count,
            vmsa_count: result.measurement.vmsa_count,
        })
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
    let igvm_hash = if args.skip_igvm {
        String::new()
    } else {
        let h = manifest::sha256_file(&igvm_path)?;
        println!("igvm     {}", h);
        h
    };
    let uki_hash = manifest::sha256_file(&output_uki)?;
    println!("uki      {}", uki_hash);

    println!("\n=== Writing manifest.json ===");
    // Write manifest
    let build_manifest = BuildManifest {
        version: 1,
        build: BuildConfig {
            timestamp: chrono_now(),
            smp: args.smp,
            memory: args.memory.clone(),
            format: "raw".to_string(),
            platform: if args.skip_igvm {
                "generic".to_string()
            } else {
                "snp".to_string()
            },
        },
        inputs: ManifestInputs {
            kernel: Some(crate::manifest::KernelInputs {
                linux_version: kernel.linux_version.clone(),
                vmlinuz_sha256: kernel.manifest.outputs.vmlinuz_sha256.clone(),
                required_config_sha256: kernel.manifest.inputs.required_config_sha256.clone(),
                hardening_config_sha256: kernel.manifest.inputs.hardening_config_sha256.clone(),
                snapshot_config_sha256: kernel.manifest.inputs.snapshot_config_sha256.clone(),
            }),
            initrd: FileEntry {
                path: initrd_path.to_string_lossy().to_string(),
                sha256: initrd_hash,
            },
            firmware: firmware
                .as_ref()
                .map(|fw| -> anyhow::Result<FileEntry> {
                    Ok(FileEntry {
                        path: fw.to_string_lossy().to_string(),
                        sha256: manifest::sha256_file(fw)?,
                    })
                })
                .transpose()?,
            base_image: FileEntry {
                path: base_abs.to_string_lossy().to_string(),
                sha256: disk_checksum.to_owned(),
            },
        },
        outputs: ManifestOutputs {
            disk_image: FileEntry {
                path: disk_path.to_string_lossy().to_string(),
                sha256: disk_checksum,
            },
            igvm: if args.skip_igvm {
                None
            } else {
                Some(FileEntry {
                    path: igvm_path.to_string_lossy().to_string(),
                    sha256: igvm_hash,
                })
            },
            uki: FileEntry {
                path: output_uki.to_string_lossy().to_string(),
                sha256: uki_hash,
            },
        },
        measurement,
    };
    let manifest_path = output.join("manifest.json");
    manifest::write_manifest(&build_manifest, &manifest_path)?;

    println!("\n===============================");
    println!("  Build complete!");
    println!("  Output:     {}", output.display());
    if !args.skip_igvm {
        println!("  IGVM:       {}", igvm_path.display());
    }
    println!("  Disk:       {}", disk_path.display());
    println!("  Manifest:   {}", manifest_path.display());
    println!("  Root hash:  {roothash}");
    if let Some(ref m) = build_manifest.measurement {
        println!("  Launch digest: {}", m.snp_launch_digest);
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

/// RAII guard to clean up debug autologin drop-in after mkosi build.
struct ConsoleCleanup {
    dir: PathBuf,
}

impl Drop for ConsoleCleanup {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.dir);
    }
}

/// Inject cloud-init user-data into the mkosi.extra seed directory.
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

/// RAII guard to clean up injected cloud-init files after mkosi build.
/// These files are only needed during the build — mkosi copies them into the image.
struct CloudInitCleanup {
    seed_dir: PathBuf,
}

impl Drop for CloudInitCleanup {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.seed_dir);
        // Walk up and remove empty parent dirs (nocloud/seed/cloud/lib/var)
        let mut dir = self.seed_dir.clone();
        while let Some(parent) = dir.parent() {
            if dir.ends_with("mkosi.extra") {
                break;
            }
            if fs_err::remove_dir(parent).is_err() {
                break; // not empty or doesn't exist
            }
            dir = parent.to_path_buf();
        }
    }
}

/// RAII guard that removes the pre-staged vmlinuz and prunes empty parent dirs
/// back up to mkosi.extra/. Mirrors CloudInitCleanup's behavior.
struct KernelStageCleanup {
    staged: PathBuf,
}

impl Drop for KernelStageCleanup {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.staged);
        let mut dir = self.staged.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            if d.ends_with("mkosi.extra") {
                break;
            }
            if fs_err::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
}

fn human_size(path: &Path) -> anyhow::Result<String> {
    let bytes = fs_err::metadata(path)?.len();
    Ok(humansize::format_size(bytes, humansize::BINARY))
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
