use std::path::{Path, PathBuf};

use crate::igvm::invoke::IgvmBuildArgs;
use crate::manifest::{
    self, BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs,
};
use crate::{tools, SealArgs};

pub fn run(args: &SealArgs) -> anyhow::Result<()> {
    tracing::info!("sealing base image with dm-verity + UKI");

    let firmware = if args.skip_igvm {
        None
    } else {
        let fw = args.firmware.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--firmware is required (or set STEEP_FIRMWARE). Pass --skip-igvm to build without IGVM.")
        })?;
        if !fw.exists() {
            anyhow::bail!("firmware not found: {}. Pass --skip-igvm to build without IGVM.", fw.display());
        }
        Some(fw.clone())
    };
    let igvm_tools = if args.skip_igvm {
        None
    } else {
        let it = args.igvm_tools.as_ref().ok_or_else(|| {
            anyhow::anyhow!("--igvm-tools is required (or set STEEP_IGVM_TOOLS). Pass --skip-igvm to build without IGVM.")
        })?;
        if !it.exists() {
            anyhow::bail!("igvm-tools not found at {}. Build it, pass --igvm-tools, or use --skip-igvm.", it.display());
        }
        Some(it.clone())
    };

    // Validate cloud-init user-data if provided
    if let Some(ref ci) = args.cloud_init {
        if !ci.exists() {
            anyhow::bail!("cloud-init user-data not found: {}", ci.display());
        }
    }

    // Check required tools
    tools::require("mkosi")?;
    tracing::info!("all required tools found");

    // Prepare output directory
    if fs_err::exists(&args.output)? {
        fs_err::remove_dir_all(&args.output)?;
    }
    fs_err::create_dir_all(&args.output)?;
    let output = args.output.canonicalize()?;

    // Inject cloud-init user-data into mkosi.extra seed directory (measured in verity root)
    let seed_dir = PathBuf::from("mkosi/base/mkosi.extra/var/lib/cloud/seed/nocloud");
    let _cloud_init_guard = if let Some(ref ci) = args.cloud_init {
        inject_cloud_init(ci, &seed_dir, args.bake)?;
        Some(CloudInitCleanup { seed_dir })
    } else {
        None
    };

    // Step 1: Build the verity initrd via mkosi (declarative)
    println!("\n=== Step 1/3: Building verity initrd (mkosi) ===");
    let initrd_dir = PathBuf::from("mkosi/initrd");
    if !initrd_dir.exists() {
        anyhow::bail!("mkosi initrd config not found: {}", initrd_dir.display());
    }
    tools::run_command_streaming(
        "sudo",
        &[
            "env",
            &format!("PATH={}", tools::safe_path()),
            "mkosi",
            "--directory",
            &initrd_dir.to_string_lossy(),
            "--force",
        ],
    )?;
    let initrd_path = initrd_dir.join("image.cpio.gz").canonicalize()?;
    println!(
        "Initrd: {} ({})",
        initrd_path.display(),
        human_size(&initrd_path)?
    );

    // Step 2: Run mkosi — builds disk with verity, UKI with roothash + our initrd + modules
    println!("\n=== Step 2/3: Building image with mkosi (verity + UKI) ===");
    let mkosi_dir = PathBuf::from("mkosi/base");
    if !mkosi_dir.exists() {
        anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
    }

    tools::run_command_streaming(
        "sudo",
        &[
            "env",
            &format!("PATH={}", tools::safe_path()),
            "mkosi",
            "--directory",
            &mkosi_dir.to_string_lossy(),
            "--force",
            "--initrd",
            &initrd_path.to_string_lossy(),
        ],
    )?;

    // Find the split artifacts mkosi produced
    let uki_path = mkosi_dir.join("image.efi");
    if !uki_path.exists() {
        anyhow::bail!("UKI .efi not found in mkosi output. Check mkosi build logs.");
    }
    let base_image = mkosi_dir.join("image.raw");
    if !base_image.exists() {
        anyhow::bail!("image.raw not found in mkosi output. Check mkosi build logs.");
    }

    // Copy UKI to output
    let output_uki = output.join("uki.efi");
    tools::sudo_copy(&uki_path, &output_uki)?;

    // Read roothash (produced by mkosi SplitArtifacts=roothash)
    let roothash_path = mkosi_dir.join("image.roothash");
    if !roothash_path.exists() {
        anyhow::bail!("image.roothash not found — check mkosi.conf has SplitArtifacts=roothash");
    }
    tools::sudo_chmod_readable(&roothash_path)?;
    let roothash = fs_err::read_to_string(&roothash_path)?.trim().to_string();
    if roothash.is_empty() || !roothash.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("invalid roothash from mkosi: {roothash:?}");
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
        println!("\n=== Step 3/3: Skipping IGVM (--skip-igvm) ===");
        None
    } else {
        println!("\n=== Step 3/3: Building IGVM ===");
        let igvm_manifest_path = output.join("igvm-manifest.json");

        // firmware and igvm_tools are guaranteed Some when skip_igvm is false (validated at top)
        let igvm_args = IgvmBuildArgs {
            igvm_tools_bin: igvm_tools.clone()
                .ok_or_else(|| anyhow::anyhow!("igvm-tools path required for IGVM build"))?,
            firmware: firmware.clone()
                .ok_or_else(|| anyhow::anyhow!("firmware path required for IGVM build"))?,
            kernel: output_uki.clone(),
            smp: args.smp,
            manifest: Some(igvm_manifest_path.clone()),
            output: igvm_path.clone(),
        };
        crate::igvm::invoke::build(&igvm_args)?;
        println!(
            "IGVM: {} ({})",
            igvm_path.display(),
            human_size(&igvm_path)?
        );

        let igvm_manifest_json = fs_err::read_to_string(&igvm_manifest_path)?;
        Some(manifest::parse_igvm_manifest(&igvm_manifest_json)?)
    };

    // Copy raw disk image to output
    let disk_path = output.join("disk.raw");
    let base_abs = base_image.canonicalize()?;
    tools::sudo_copy(&base_abs, &disk_path)?;

    // Write manifest
    let build_manifest = BuildManifest {
        version: 1,
        build: BuildConfig {
            timestamp: chrono_now(),
            smp: args.smp,
            memory: args.memory.clone(),
            format: "raw".to_string(),
            platform: if args.skip_igvm { "generic".to_string() } else { "snp".to_string() },
        },
        inputs: ManifestInputs {
            initrd: FileEntry {
                path: initrd_path.to_string_lossy().to_string(),
                sha256: manifest::sha256_file(&initrd_path)?,
            },
            firmware: firmware.as_ref().map(|fw| -> anyhow::Result<FileEntry> {
                Ok(FileEntry {
                    path: fw.to_string_lossy().to_string(),
                    sha256: manifest::sha256_file(fw)?,
                })
            }).transpose()?,
            base_image: FileEntry {
                path: base_abs.to_string_lossy().to_string(),
                sha256: manifest::sha256_file(&base_abs)?,
            },
        },
        outputs: ManifestOutputs {
            disk_image: FileEntry {
                path: disk_path.to_string_lossy().to_string(),
                sha256: manifest::sha256_file(&disk_path)?,
            },
            igvm: if args.skip_igvm {
                None
            } else {
                Some(FileEntry {
                    path: igvm_path.to_string_lossy().to_string(),
                    sha256: manifest::sha256_file(&igvm_path)?,
                })
            },
            uki: FileEntry {
                path: output_uki.to_string_lossy().to_string(),
                sha256: manifest::sha256_file(&output_uki)?,
            },
        },
        measurement,
    };
    let manifest_path = output.join("manifest.json");
    manifest::write_manifest(&build_manifest, &manifest_path)?;

    println!("\n===============================");
    println!("  Seal complete!");
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
        println!("  Cloud-init: measured in verity root{}", if args.bake { " (baked)" } else { " (boot-time)" });
    }
    println!("===============================");

    Ok(())
}

/// Inject cloud-init user-data into the mkosi.extra seed directory.
/// The NoCloud datasource picks up user-data from /var/lib/cloud/seed/nocloud/.
/// When bake=true, a sentinel file is also written so mkosi.finalize runs cloud-init.
fn inject_cloud_init(user_data: &Path, seed_dir: &Path, bake: bool) -> anyhow::Result<()> {
    fs_err::create_dir_all(seed_dir)?;

    // Copy user-data
    fs_err::copy(user_data, seed_dir.join("user-data"))?;

    // Create minimal meta-data
    fs_err::write(
        seed_dir.join("meta-data"),
        "instance-id: steep-sealed\nlocal-hostname: steep\n",
    )?;

    if bake {
        // Sentinel tells mkosi.finalize to run cloud-init in chroot
        fs_err::write(seed_dir.join(".steep-bake"), "")?;
        println!("Cloud-init: will be applied at build time (--bake)");
    } else {
        println!("Cloud-init: config measured in image, will run at boot");
    }

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
            if parent.ends_with("mkosi.extra") {
                break;
            }
            if fs_err::remove_dir(parent).is_err() {
                break; // not empty or doesn't exist
            }
            dir = parent.to_path_buf();
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
