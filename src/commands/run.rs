use crate::manifest::{self, BuildManifest};
use crate::qemu::{self, QemuArgs, QemuTier};
use crate::RunArgs;

const ALLOWED_DISK_FORMATS: &[&str] = &["raw", "qcow2"];
const ALLOWED_PLATFORMS: &[&str] = &["snp", "generic"];

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "launching VM");

    if !args.dir.exists() {
        anyhow::bail!("output directory not found: {}", args.dir.display());
    }

    // Read manifest
    let manifest_path = args.dir.join("manifest.json");
    if !manifest_path.exists() {
        anyhow::bail!(
            "manifest.json not found in {}. Run `steep seal` first.",
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

    // Resolve artifacts based on tier
    let igvm_path;
    let uki_path;
    let firmware_path;

    // Default to the first variant (smallest SMP after sort, or the build-time
    // default if `steep igvm` was never run). A future change can add a `--smp`
    // selector to `steep run`; for now this matches v1 behaviour of "one IGVM
    // per output dir."
    let variant = manifest.variants.first();

    match tier {
        QemuTier::SevSnp => {
            let v = variant.ok_or_else(|| {
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
            igvm_path = Some(path);
            uki_path = None;
            firmware_path = None;
        }
        QemuTier::Kvm | QemuTier::Emulated => {
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
            igvm_path = None;
            uki_path = Some(uki);
            firmware_path = Some(fw);
        }
    }

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

    // SMP comes from the selected variant; if no variants exist (skip_igvm
    // builds running on KVM/emulated), fall back to a sensible default.
    let smp = variant.map(|v| v.smp).unwrap_or(2);

    // Launch
    let qemu_args = QemuArgs {
        tier,
        qemu_bin: args.qemu_bin.clone(),
        igvm: igvm_path,
        uki: uki_path,
        firmware: firmware_path,
        disk: disk_path,
        disk_format: manifest.build.format,
        smp,
        memory: manifest.build.memory,
        port_forwards,
        scratch: scratch_path,
    };

    println!(
        "Launching VM (smp={}, memory={}, tier={:?})",
        qemu_args.smp, qemu_args.memory, qemu_args.tier
    );
    if let Some(v) = variant {
        println!("Launch digest: {}", v.measurement.snp_launch_digest);
    }

    qemu::launch(&qemu_args)?;
    Ok(())
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
    for v in &manifest.variants {
        if v.smp == 0 || v.smp > 1024 {
            anyhow::bail!(
                "invalid smp count in manifest variant: {} (must be 1-1024)",
                v.smp
            );
        }
    }
    Ok(())
}
