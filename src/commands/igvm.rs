use crate::manifest::{self, BuildManifest, FileEntry, Measurement, SnpVariant};
use crate::IgvmArgs;

/// Generate IGVM files for multiple SMP counts from an existing sealed output.
///
/// Reads the UKI from the sealed output directory and the firmware from the
/// provided path, then builds one IGVM per SMP count. Each invocation is
/// idempotent per SMP value: if `variants[]` in the manifest already contains
/// an entry for SMP=N whose recorded SHA-256 still matches the on-disk IGVM
/// file, the build is skipped. Otherwise the variant is (re)built and the
/// manifest entry replaced.
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
            "uki.efi not found in {}. Run `confos build` first.",
            args.dir.display()
        );
    }

    // The manifest must already exist — `confos igvm` mutates an existing build,
    // it doesn't create one from scratch. (Without a manifest we'd have nowhere
    // to record measurements.)
    let manifest_path = args.dir.join("manifest.json");
    if !manifest_path.exists() {
        anyhow::bail!(
            "manifest.json not found in {}. Run `confos build` first.",
            args.dir.display()
        );
    }
    let mut build_manifest = manifest::read_manifest(&manifest_path)?;

    // Check sealed inputs before reusing an existing measured variant. A
    // valid old IGVM hash alone cannot establish that these inputs match it.
    let (fw_bytes, uki_bytes) = prepare_inputs(args, &mut build_manifest)?;

    println!("Generating IGVM files for SMP counts: {:?}", args.smp);

    for &smp in &args.smp {
        if smp == 0 {
            anyhow::bail!("SMP count must be >= 1");
        }

        let igvm_name = format!("guest-smp{smp}.igvm");
        let igvm_path = args.dir.join(&igvm_name);

        print!("  smp={smp} ... ");

        // Idempotency: if a variant already exists for this SMP, its IGVM file
        // is on disk, and the recorded sha256 matches, skip the rebuild.
        if let Some(existing) = build_manifest.snp_variants.iter().find(|v| v.smp == smp) {
            if igvm_path.exists() {
                match manifest::sha256_file(&igvm_path) {
                    Ok(actual) if actual == existing.igvm.sha256 => {
                        println!(
                            "{} unchanged (sha256 match, digest: {})",
                            igvm_name, existing.measurement.snp_launch_digest
                        );
                        continue;
                    }
                    Ok(_) => {
                        println!("(file present but sha256 mismatch — rebuilding)");
                    }
                    Err(e) => {
                        println!("(could not hash existing file: {e} — rebuilding)");
                    }
                }
            } else {
                println!("(file missing — rebuilding)");
            }
        }

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

        let variant = SnpVariant {
            smp,
            igvm: FileEntry {
                path: igvm_name,
                sha256: manifest::sha256_file(&igvm_path)?,
            },
            measurement: Measurement {
                snp_launch_digest: digest,
                algorithm: "sha384".to_string(),
                page_count: result.measurement.page_count,
                vmsa_count: result.measurement.vmsa_count,
            },
        };
        upsert_variant(&mut build_manifest, variant);
    }

    // Persist updates. Sort variants by SMP for stable on-disk ordering, so a
    // diff between two manifest.json files is meaningful even if the user
    // happened to invoke `--smp 8 2 4` versus `--smp 2 4 8`.
    build_manifest.snp_variants.sort_by_key(|v| v.smp);
    build_manifest.build.platform = if build_manifest.tdx.is_some() {
        "both"
    } else {
        "snp"
    }
    .to_string();
    manifest::write_manifest(&build_manifest, &manifest_path)?;

    println!("\nDone. IGVM files written to {}", args.dir.display());
    Ok(())
}

fn prepare_inputs(
    args: &IgvmArgs,
    build: &mut BuildManifest,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let uki = args.dir.join("uki.efi");
    if manifest::sha256_file(&uki)? != build.outputs.uki.sha256 {
        anyhow::bail!(
            "UKI differs from the sealed manifest; build a new image before deriving SNP variants"
        );
    }
    let firmware_hash = manifest::sha256_file(&args.firmware)?;
    if let Some(recorded) = &build.inputs.firmware {
        if recorded.sha256 != firmware_hash {
            anyhow::bail!("SNP firmware differs from the sealed manifest; refusing to reuse its launch measurements");
        }
    } else if !build.snp_variants.is_empty() {
        anyhow::bail!("existing SNP variants have no recorded firmware identity; derive them again from the original sealed output");
    }
    let output_firmware = args.dir.join("OVMF.fd");
    if output_firmware.exists() {
        if manifest::sha256_file(&output_firmware)? != firmware_hash {
            anyhow::bail!("output OVMF.fd differs from the supplied SNP firmware");
        }
    } else {
        fs_err::copy(&args.firmware, &output_firmware)?;
    }
    build.inputs.firmware = Some(FileEntry {
        path: "OVMF.fd".into(),
        sha256: firmware_hash,
    });
    Ok((fs_err::read(&output_firmware)?, fs_err::read(&uki)?))
}

/// Replace any existing variant with the same SMP count, or append if absent.
/// Ensures `variants[]` has at most one entry per SMP.
pub(crate) fn upsert_variant(manifest: &mut BuildManifest, variant: SnpVariant) {
    if let Some(existing) = manifest
        .snp_variants
        .iter_mut()
        .find(|v| v.smp == variant.smp)
    {
        *existing = variant;
    } else {
        manifest.snp_variants.push(variant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs, MANIFEST_VERSION,
    };

    fn entry(s: &str) -> FileEntry {
        FileEntry {
            path: s.to_string(),
            sha256: format!("sha-{s}"),
        }
    }

    fn empty_manifest() -> BuildManifest {
        BuildManifest {
            version: MANIFEST_VERSION,
            build: BuildConfig {
                timestamp: "t".into(),
                memory: "2G".into(),
                format: "raw".into(),
                platform: "snp".into(),
            },
            inputs: ManifestInputs {
                kernel: None,
                initrd: entry("initrd"),
                firmware: Some(entry("fw")),
                base_image: entry("base"),
            },
            outputs: ManifestOutputs {
                disk_image: entry("disk"),
                uki: entry("uki"),
            },
            snp_variants: vec![],
            tdx: None,
        }
    }

    fn variant(smp: u32, digest: &str) -> SnpVariant {
        SnpVariant {
            smp,
            igvm: FileEntry {
                path: format!("guest-smp{smp}.igvm"),
                sha256: format!("igvm-sha-{smp}-{digest}"),
            },
            measurement: Measurement {
                snp_launch_digest: digest.to_string(),
                algorithm: "sha384".to_string(),
                page_count: 100,
                vmsa_count: smp,
            },
        }
    }

    #[test]
    fn deriving_snp_records_firmware_and_rejects_sealed_input_drift() {
        let directory = tempfile::TempDir::new().unwrap();
        let external = tempfile::TempDir::new().unwrap();
        let uki = directory.path().join("uki.efi");
        let firmware = external.path().join("firmware.fd");
        fs_err::write(&uki, b"SEALED-UKI").unwrap();
        fs_err::write(&firmware, b"PINNED-SNP-FIRMWARE").unwrap();
        let args = IgvmArgs {
            dir: directory.path().to_owned(),
            firmware: firmware.clone(),
            smp: vec![2],
        };
        let mut build = empty_manifest();
        build.inputs.firmware = None;
        build.outputs.uki.sha256 = manifest::sha256_file(&uki).unwrap();
        let inputs = prepare_inputs(&args, &mut build).unwrap();
        assert_eq!(
            inputs,
            (b"PINNED-SNP-FIRMWARE".to_vec(), b"SEALED-UKI".to_vec())
        );
        assert_eq!(build.inputs.firmware.as_ref().unwrap().path, "OVMF.fd");
        assert_eq!(
            fs_err::read(directory.path().join("OVMF.fd")).unwrap(),
            b"PINNED-SNP-FIRMWARE"
        );
        prepare_inputs(&args, &mut build).unwrap();

        fs_err::write(&firmware, b"OTHER-FIRMWARE").unwrap();
        assert!(prepare_inputs(&args, &mut build)
            .unwrap_err()
            .to_string()
            .contains("SNP firmware differs"));
        fs_err::write(&firmware, b"PINNED-SNP-FIRMWARE").unwrap();
        fs_err::write(&uki, b"UNSEALED-UKI").unwrap();
        assert!(prepare_inputs(&args, &mut build)
            .unwrap_err()
            .to_string()
            .contains("UKI differs"));
    }

    #[test]
    fn existing_variants_without_firmware_provenance_fail_closed() {
        let directory = tempfile::TempDir::new().unwrap();
        let uki = directory.path().join("uki.efi");
        let firmware = directory.path().join("firmware.fd");
        fs_err::write(&uki, b"UKI").unwrap();
        fs_err::write(&firmware, b"FW").unwrap();
        let args = IgvmArgs {
            dir: directory.path().to_owned(),
            firmware,
            smp: vec![2],
        };
        let mut build = empty_manifest();
        build.outputs.uki.sha256 = manifest::sha256_file(&uki).unwrap();
        build.inputs.firmware = None;
        build.snp_variants.push(variant(2, "old"));
        assert!(prepare_inputs(&args, &mut build)
            .unwrap_err()
            .to_string()
            .contains("no recorded firmware identity"));
    }

    #[test]
    fn upsert_appends_new_variant() {
        let mut m = empty_manifest();
        upsert_variant(&mut m, variant(2, "d2"));
        assert_eq!(m.snp_variants.len(), 1);
        assert_eq!(m.snp_variants[0].smp, 2);
    }

    #[test]
    fn upsert_replaces_matching_smp() {
        // Idempotency: calling `confos igvm --smp 2` twice must not produce two
        // entries with smp=2 in the manifest.
        let mut m = empty_manifest();
        upsert_variant(&mut m, variant(2, "d2-old"));
        upsert_variant(&mut m, variant(2, "d2-new"));
        assert_eq!(m.snp_variants.len(), 1);
        assert_eq!(m.snp_variants[0].measurement.snp_launch_digest, "d2-new");
    }

    #[test]
    fn upsert_keeps_distinct_smp_variants() {
        // `confos igvm --smp 2 4` must produce two distinct variants. They keep
        // their distinct launch digests after a no-op replacement.
        let mut m = empty_manifest();
        upsert_variant(&mut m, variant(2, "d2"));
        upsert_variant(&mut m, variant(4, "d4"));
        upsert_variant(&mut m, variant(2, "d2")); // re-upsert same smp
        assert_eq!(m.snp_variants.len(), 2);
        let digests: Vec<&str> = m
            .snp_variants
            .iter()
            .map(|v| v.measurement.snp_launch_digest.as_str())
            .collect();
        assert!(digests.contains(&"d2"));
        assert!(digests.contains(&"d4"));
    }
}
