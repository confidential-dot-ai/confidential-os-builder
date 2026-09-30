use std::path::PathBuf;

use crate::manifest::{self, BuildManifest, SnpVariant};
use crate::qemu::{self, QemuArgs, QemuTier};
use crate::RunArgs;

const ALLOWED_DISK_FORMATS: &[&str] = &["raw", "qcow2"];
// `build.platform` values that `confos run` knows how to launch. Keep in
// sync with commands::build::run() — adding a new BuildPlatform variant
// there without teaching the runner about its hardware tier produces a
// misleading "unsupported" error.
const ALLOWED_PLATFORMS: &[&str] = &["snp", "tdx", "multi", "generic"];

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    qemu::validate_cdroms(&args.cdroms)?;
    tracing::info!(dir = %args.dir.display(), "launching VM");

    if !args.dir.exists() {
        anyhow::bail!("output directory not found: {}", args.dir.display());
    }

    // Read manifest
    let manifest_path = args.dir.join("manifest.json");
    if !manifest_path.exists() {
        anyhow::bail!(
            "manifest.json not found in {}. Run `confos build` first.",
            args.dir.display()
        );
    }
    let manifest = manifest::read_manifest(&manifest_path)?;

    // Validate manifest-derived values before they reach QEMU argument interpolation.
    // These fields are comma-interpolated into QEMU -object/-drive args where commas
    // are delimiters, so injection is possible without validation.
    validate_manifest_fields(&manifest)?;

    // Detect QEMU tier
    let tier = qemu::detect_tier_for(&args.qemu_bin)?;
    match tier {
        QemuTier::SevSnp => {
            println!("QEMU tier: SEV-SNP (confidential computing)");
        }
        QemuTier::Kvm => {
            eprintln!("WARNING: QEMU lacks IGVM/SEV-SNP support. Running with KVM acceleration only — no confidential computing guarantees.");
        }
        QemuTier::Emulated => {
            eprintln!("WARNING: Neither SEV-SNP nor KVM available. Running in pure emulation mode — this will be slow.");
        }
    }

    let boot = resolve_boot_config(args, &manifest, tier)?;

    // Find disk image
    let disk_path = args.dir.join(format!("disk.{}", manifest.build.format));
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.{} not found in {}",
            manifest.build.format,
            args.dir.display()
        );
    }

    // Parse port forwards
    let port_forwards = args
        .port_forward
        .iter()
        .map(|s| {
            let (host_str, guest_str) = s.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("invalid --port-forward format, expected HOST:GUEST: {s}")
            })?;
            let host = host_str
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid host port: {host_str}"))?;
            let guest = guest_str
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid guest port: {guest_str}"))?;
            Ok((host, guest))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Optional ephemeral scratch disk: create a sparse raw file of the
    // requested size and hand it to QEMU. The disk is attached with
    // `serial=confai-scratch` (see qemu.rs) so the initrd recognizes it via
    // /sys/block/<dev>/serial without needing a pre-existing filesystem.
    // No mkfs here — the initrd opens it under cryptsetup and runs its own
    // mkfs on the encrypted device on every boot.
    let scratch_path = match args.scratch {
        Some(ref size) => {
            let bytes = qemu::parse_size_to_bytes(size)?;
            let path = args.dir.join("scratch.raw");
            let f = fs_err::File::create(&path)?;
            f.set_len(bytes)?;
            drop(f);
            println!(
                "Created ephemeral scratch disk ({size}) at {}",
                path.display()
            );
            Some(path)
        }
        None => None,
    };

    // Launch
    let qemu_args = QemuArgs {
        tier,
        qemu_bin: args.qemu_bin.clone(),
        igvm: boot.igvm,
        uki: boot.uki,
        firmware: boot.firmware,
        disk: disk_path,
        disk_format: manifest.build.format,
        smp: boot.smp,
        memory: manifest.build.memory,
        port_forwards,
        scratch: scratch_path,
        host_data: args.host_data,
        cdroms: args.cdroms.clone(),
    };

    println!(
        "Launching VM (smp={}, memory={}, tier={:?})",
        qemu_args.smp, qemu_args.memory, qemu_args.tier
    );
    if let Some(digest) = boot.snp_launch_digest {
        println!("Launch digest: {digest}");
    }
    if let Some(data) = args.host_data {
        println!("HOST_DATA: {}", hex::encode(data));
    }

    qemu::launch(&qemu_args)?;
    Ok(())
}

struct BootConfig {
    igvm: Option<PathBuf>,
    uki: Option<PathBuf>,
    firmware: Option<PathBuf>,
    smp: u32,
    snp_launch_digest: Option<String>,
}

fn resolve_boot_config(
    args: &RunArgs,
    manifest: &BuildManifest,
    tier: QemuTier,
) -> anyhow::Result<BootConfig> {
    match tier {
        QemuTier::SevSnp => {
            let v = select_variant(&manifest.snp_variants, args.smp)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "no IGVM variants in manifest at {}. Was the image built with --skip-igvm?",
                    args.dir.display()
                )
            })?;
            let path = args.dir.join(&v.igvm.path);
            if !path.exists() {
                anyhow::bail!(
                    "{} not found in {} (referenced by manifest variant smp={})",
                    v.igvm.path,
                    args.dir.display(),
                    v.smp,
                );
            }
            Ok(BootConfig {
                igvm: Some(path),
                uki: None,
                firmware: None,
                smp: v.smp,
                snp_launch_digest: Some(v.measurement.snp_launch_digest.clone()),
            })
        }
        QemuTier::Kvm | QemuTier::Emulated => {
            anyhow::ensure!(
                args.host_data.is_none(),
                "--host-data requires the SEV-SNP tier"
            );
            let uki = args.dir.join("uki.efi");
            if !uki.exists() {
                anyhow::bail!("uki.efi not found in {}", args.dir.display());
            }
            let fw = if let Some(ref cli_fw) = args.firmware {
                if !cli_fw.exists() {
                    anyhow::bail!("firmware not found: {}", cli_fw.display());
                }
                cli_fw.clone()
            } else if manifest.inputs.firmware.is_some() {
                let fw = args.dir.join("OVMF.fd");
                if !fw.exists() {
                    anyhow::bail!(
                        "firmware not found at {} (build copies firmware into the output directory)",
                        fw.display()
                    );
                }
                fw
            } else {
                anyhow::bail!(
                    "no firmware available — image was built with --skip-igvm. Pass --firmware <path> to run on KVM."
                );
            };
            Ok(BootConfig {
                igvm: None,
                uki: Some(uki),
                firmware: Some(fw),
                smp: args
                    .smp
                    .unwrap_or_else(|| manifest.snp_variants.first().map_or(2, |v| v.smp)),
                snp_launch_digest: None,
            })
        }
    }
}

fn select_variant(
    variants: &[SnpVariant],
    smp: Option<u32>,
) -> anyhow::Result<Option<&SnpVariant>> {
    let Some(smp) = smp else {
        return Ok(variants.first());
    };
    if variants.is_empty() {
        return Ok(None);
    }
    if let Some(variant) = variants.iter().find(|variant| variant.smp == smp) {
        return Ok(Some(variant));
    }
    let available = variants
        .iter()
        .map(|variant| variant.smp.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!("no SNP variant for --smp {smp} (manifest has: {available}); add one with: confos igvm <dir> --smp {smp}")
}

/// Validate manifest fields that flow into QEMU arguments or path construction.
/// Prevents injection via comma-delimited QEMU args and path traversal via format field.
fn validate_manifest_fields(manifest: &BuildManifest) -> anyhow::Result<()> {
    if !ALLOWED_DISK_FORMATS.contains(&manifest.build.format.as_str()) {
        anyhow::bail!(
            "unsupported disk format in manifest: {:?} (allowed: {:?})",
            manifest.build.format,
            ALLOWED_DISK_FORMATS
        );
    }
    if !ALLOWED_PLATFORMS.contains(&manifest.build.platform.as_str()) {
        anyhow::bail!(
            "unsupported platform in manifest: {:?} (allowed: {:?})",
            manifest.build.platform,
            ALLOWED_PLATFORMS
        );
    }
    qemu::validate_memory(&manifest.build.memory)?;
    for v in &manifest.snp_variants {
        if v.smp == 0 || v.smp > 1024 {
            anyhow::bail!(
                "invalid smp count in manifest variant: {} (must be 1-1024)",
                v.smp
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        BuildConfig, FileEntry, ManifestInputs, ManifestOutputs, Measurement, MANIFEST_VERSION,
    };

    fn variants() -> Vec<SnpVariant> {
        [2, 4, 8, 16]
            .into_iter()
            .map(|smp| SnpVariant {
                smp,
                igvm: FileEntry {
                    path: format!("guest-smp{smp}.igvm"),
                    sha256: String::new(),
                },
                measurement: Measurement {
                    snp_launch_digest: format!("digest{smp}"),
                    algorithm: "sha384".into(),
                    page_count: 1,
                    vmsa_count: smp,
                },
            })
            .collect()
    }

    #[test]
    fn snp_boot_config_uses_one_variant() {
        let dir = tempfile::tempdir().unwrap();
        let igvm = dir.path().join("guest-smp8.igvm");
        std::fs::write(&igvm, []).unwrap();
        let args = RunArgs {
            dir: dir.path().to_path_buf(),
            smp: Some(8),
            host_data: None,
            cdroms: vec![],
            scratch: None,
            port_forward: vec![],
            qemu_bin: "unused-qemu".into(),
            firmware: None,
        };
        let entry = FileEntry {
            path: String::new(),
            sha256: String::new(),
        };
        let manifest = BuildManifest {
            version: MANIFEST_VERSION,
            build: BuildConfig {
                timestamp: String::new(),
                memory: "2G".into(),
                format: "raw".into(),
                platform: "snp".into(),
            },
            inputs: ManifestInputs {
                kernel: None,
                initrd: entry.clone(),
                firmware: None,
                base_image: entry.clone(),
            },
            outputs: ManifestOutputs {
                disk_image: entry.clone(),
                uki: entry,
            },
            snp_variants: variants(),
            tdx: None,
        };
        let boot = resolve_boot_config(&args, &manifest, QemuTier::SevSnp).unwrap();
        assert_eq!(boot.igvm.as_ref(), Some(&igvm));
        assert_eq!(boot.smp, 8);
        assert_eq!(boot.snp_launch_digest.unwrap(), "digest8");
        assert!(boot.uki.is_none());
        assert!(boot.firmware.is_none());
    }

    #[test]
    fn select_variant_matches_cpu_count() {
        let variants = variants();
        let selected = select_variant(&variants, Some(8)).unwrap().unwrap();
        assert_eq!(selected.smp, 8);
        assert_eq!(selected.igvm.path, "guest-smp8.igvm");
        assert_eq!(selected.measurement.snp_launch_digest, "digest8");
    }

    #[test]
    fn select_variant_lists_available_counts_on_mismatch() {
        assert_eq!(select_variant(&variants(), Some(3)).unwrap_err().to_string(),
            "no SNP variant for --smp 3 (manifest has: 2, 4, 8, 16); add one with: confos igvm <dir> --smp 3");
    }

    #[test]
    fn select_variant_defaults_to_first_entry() {
        let mut variants = variants();
        variants.swap(0, 1);
        assert_eq!(select_variant(&variants, None).unwrap().unwrap().smp, 4);
    }

    #[test]
    fn select_variant_accepts_empty_variants() {
        assert!(select_variant(&[], Some(4)).unwrap().is_none());
        assert!(select_variant(&[], None).unwrap().is_none());
    }
}
