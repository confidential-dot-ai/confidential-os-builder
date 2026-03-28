use std::path::PathBuf;

use crate::manifest;
use crate::qemu::{QemuArgs, QemuTier};
use crate::RunArgs;

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "launching VM");

    // Step 1: Validate directory exists
    if !args.dir.exists() {
        anyhow::bail!("output directory not found: {}", args.dir.display());
    }

    // Step 2: Read manifest
    let manifest_path = args.dir.join("manifest.json");
    if !manifest_path.exists() {
        anyhow::bail!("manifest.json not found in {}", args.dir.display());
    }
    let manifest = manifest::read_manifest(&manifest_path)?;

    // Step 3: Detect QEMU tier
    let tier = crate::qemu::detect_tier()?;

    // Step 4: Print warnings for degraded tiers
    match tier {
        QemuTier::SevSnp => {}
        QemuTier::Kvm => {
            eprintln!("WARNING: QEMU lacks IGVM/SEV-SNP support. Running with KVM acceleration only — no confidential computing guarantees.");
        }
        QemuTier::Emulated => {
            eprintln!("WARNING: Neither SEV-SNP nor KVM available. Running in pure emulation mode — this will be slow.");
        }
    }

    // Step 5: Validate artifacts based on tier
    let igvm_path;
    let uki_path;
    let firmware_path;

    match tier {
        QemuTier::SevSnp => {
            let path = args.dir.join("guest.igvm");
            if !path.exists() {
                anyhow::bail!("guest.igvm not found in {}", args.dir.display());
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
            let fw = PathBuf::from(&manifest.inputs.firmware.path);
            if !fw.exists() {
                anyhow::bail!(
                    "firmware not found at {} (recorded in manifest). The OVMF firmware from build time must still be present.",
                    manifest.inputs.firmware.path
                );
            }
            igvm_path = None;
            uki_path = Some(uki);
            firmware_path = Some(fw);
        }
    }

    // Step 6: Find disk image using format from manifest
    let disk_path = args.dir.join(format!("disk.{}", manifest.build.format));
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.{} not found in {}",
            manifest.build.format,
            args.dir.display()
        );
    }

    // Step 8: Parse port forwards
    let port_forwards = args
        .port_forward
        .iter()
        .map(|s| {
            let (host_str, guest_str) = s.split_once(':').ok_or_else(|| {
                anyhow::anyhow!("invalid --port-forward format, expected HOST:GUEST: {s}")
            })?;
            let host = host_str
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid host port in --port-forward: {host_str}"))?;
            let guest = guest_str.parse::<u16>().map_err(|_| {
                anyhow::anyhow!("invalid guest port in --port-forward: {guest_str}")
            })?;
            Ok((host, guest))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Step 9: Launch QEMU
    let qemu_args = QemuArgs {
        tier,
        igvm: igvm_path,
        uki: uki_path,
        firmware: firmware_path,
        disk: disk_path,
        disk_format: "qcow2".to_string(),
        smp: manifest.build.smp,
        memory: manifest.build.memory,
        port_forwards,
    };
    crate::qemu::launch(&qemu_args)?;

    Ok(())
}
