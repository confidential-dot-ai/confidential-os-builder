use std::path::PathBuf;

use crate::manifest;
use crate::qemu::{self, QemuArgs, QemuTier};
use crate::RunArgs;

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

    match tier {
        QemuTier::SevSnp => {
            let path = args.dir.join("guest.igvm");
            if !path.exists() {
                anyhow::bail!("guest.igvm not found in {}. Was the image built with --skip-igvm?", args.dir.display());
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
            } else if let Some(ref fw_entry) = manifest.inputs.firmware {
                let fw = PathBuf::from(&fw_entry.path);
                if !fw.exists() {
                    anyhow::bail!(
                        "firmware not found at {} (recorded in manifest)",
                        fw_entry.path
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

    // Launch
    let qemu_args = QemuArgs {
        tier,
        qemu_bin: args.qemu_bin.clone(),
        igvm: igvm_path,
        uki: uki_path,
        firmware: firmware_path,
        disk: disk_path,
        disk_format: manifest.build.format,
        smp: manifest.build.smp,
        memory: manifest.build.memory,
        port_forwards,
    };

    println!(
        "Launching VM (smp={}, memory={}, tier={:?})",
        qemu_args.smp, qemu_args.memory, qemu_args.tier
    );
    if let Some(ref m) = manifest.measurement {
        println!("Launch digest: {}", m.snp_launch_digest);
    }

    qemu::launch(&qemu_args)?;
    Ok(())
}
