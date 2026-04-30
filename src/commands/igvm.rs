use crate::manifest::{self, Measurement};
use crate::IgvmArgs;

/// Generate IGVM files for multiple SMP counts from an existing sealed output.
///
/// Reads the UKI from the sealed output directory and the firmware from the
/// provided path, then builds one IGVM per SMP count. Existing IGVM files
/// in the output directory are preserved unless overwritten by a matching
/// SMP count.
pub fn run(args: &IgvmArgs) -> anyhow::Result<()> {
    if !args.dir.exists() {
        anyhow::bail!("output directory not found: {}", args.dir.display());
    }
    if !args.firmware.exists() {
        anyhow::bail!("firmware not found: {}", args.firmware.display());
    }

    let uki_path = args.dir.join("uki.efi");
    if !uki_path.exists() {
        anyhow::bail!(
            "uki.efi not found in {}. Run `steep seal` first.",
            args.dir.display()
        );
    }

    // Read inputs once
    let fw_bytes = fs_err::read(&args.firmware)?;
    let uki_bytes = fs_err::read(&uki_path)?;

    // Update the existing manifest if present
    let manifest_path = args.dir.join("manifest.json");
    let mut build_manifest = if manifest_path.exists() {
        Some(manifest::read_manifest(&manifest_path)?)
    } else {
        None
    };

    println!("Generating IGVM files for SMP counts: {:?}", args.smp);

    for &smp in &args.smp {
        if smp == 0 {
            anyhow::bail!("SMP count must be >= 1");
        }

        let igvm_name = format!("guest-smp{smp}.igvm");
        let igvm_path = args.dir.join(&igvm_name);

        print!("  smp={smp} ... ");

        let result = crate::igvm::invoke::build_snp(&fw_bytes, &uki_bytes, smp)?;
        fs_err::write(&igvm_path, &result.igvm_bytes)?;

        let digest = hex::encode(result.measurement.launch_digest);
        println!(
            "{} ({} pages, {} VMSAs, digest: {}...{})",
            igvm_name,
            result.measurement.page_count,
            result.measurement.vmsa_count,
            &digest[..8],
            &digest[digest.len() - 8..],
        );

        // Update manifest measurement for the first (or only) SMP count
        if let Some(ref mut m) = build_manifest {
            // Store measurement from the first SMP count in the main manifest
            // (preserves backward compat with single-IGVM manifests)
            if smp == args.smp[0] {
                m.measurement = Some(Measurement {
                    snp_launch_digest: digest.clone(),
                    algorithm: "sha384".to_string(),
                    page_count: result.measurement.page_count,
                    vmsa_count: result.measurement.vmsa_count,
                });
            }
        }
    }

    // Write updated manifest
    if let Some(ref m) = build_manifest {
        manifest::write_manifest(m, &manifest_path)?;
    }

    println!("\nDone. IGVM files written to {}", args.dir.display());
    Ok(())
}
