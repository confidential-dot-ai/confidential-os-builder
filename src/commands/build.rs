use std::path::{Path, PathBuf};

use crate::{igvm, kernel_cache, manifest, qemu, tools, BuildArgs, BuildPlatform};

pub fn run(args: &BuildArgs) -> anyhow::Result<()> {
    tracing::info!("building base image with dm-verity + UKI");

    // Resolve the requested platform set. --skip-igvm is the historical
    // way to ask for "no SNP measurement", which now corresponds to
    // `--platform tdx`. Honour it for back-compat with shell wrappers,
    // but warn so the operator migrates. Reject conflicting combos to
    // catch typos before a 10-minute build runs.
    let platform = if args.skip_igvm {
        match args.platform {
            // The legacy `--skip-igvm` and the new `--platform tdx` mean
            // the same thing; accept the combo silently as redundant
            // rather than rejecting it as a "conflict" — operators
            // migrating their wrapper scripts get a smooth path that
            // accepts both spellings.
            BuildPlatform::Tdx | BuildPlatform::Both => {}
            BuildPlatform::Snp => anyhow::bail!(
                "--skip-igvm with --platform snp is incoherent (skip-igvm \
                 produces no SNP launch digest, but --platform snp asks for one). \
                 Drop one of the flags."
            ),
        }
        eprintln!(
            "warning: --skip-igvm is deprecated; use `--platform tdx` instead. \
             treating this build as `--platform tdx`."
        );
        BuildPlatform::Tdx
    } else {
        args.platform
    };

    // SNP firmware: required only when building SNP variants. Must be
    // confos's edk2 build with IgvmHobArea — Ubuntu's stock OVMF does not
    // have that region and IGVM construction fails on it.
    let snp_firmware = if platform.needs_snp() {
        let fw = args.firmware.clone();
        if !fw.exists() {
            anyhow::bail!(
                "SNP firmware not found: {} (--firmware). Pass `--platform tdx` to build without SNP measurement.",
                fw.display()
            );
        }
        Some(fw)
    } else {
        None
    };

    // TDX firmware: required only when computing TDX measurements. Must
    // be an OVMF build with TDVF code paths (Ubuntu's `ovmf` package
    // works). confos's IGVM-aware firmware does NOT include TDVF — a TDX
    // guest booted on it hangs silently in firmware. So we keep two
    // firmware binaries side by side, one per platform.
    let tdx_firmware = if platform.needs_tdx() {
        let fw = args.tdx_firmware.clone();
        if !fw.exists() {
            anyhow::bail!(
                "TDX firmware not found: {} (--tdx-firmware). Needs the unified \
                 Intel TDX build `OVMF.inteltdx.fd` (boots a TD via -bios); \
                 ubuntu's `ovmf` package ships it under /usr/share/ovmf/. If \
                 your distro names it differently (e.g. some builds ship only \
                 OVMF.tdx.fd, which is pflash-style and does NOT -bios-boot), \
                 point --tdx-firmware / CONFOS_TDX_FIRMWARE at a -bios-capable \
                 OVMF.inteltdx.fd.",
                fw.display()
            );
        }
        Some(fw)
    } else {
        None
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

    // Don't wipe mkosi.local at start: profile sync hooks (e.g.
    // mkosi/base/mkosi.profiles/attest/mkosi.sync staging a binary into
    // mkosi.local/mkosi.extra/) must survive into the rest of the mkosi run,
    // and operator prep (bin/confos-fetch-<NAME>) stages into it *before* the
    // build. The RemoveDirOnDrop guard below removes mkosi.local on normal
    // exit; what a hard-killed build left behind is recovered by name in
    // write_sync_inputs.
    let mkosi_local = PathBuf::from("mkosi/base/mkosi.local");

    // Ahead of the guard, deliberately: write_sync_inputs is what decides
    // whether this overlay is ours to use, and its "another build owns it"
    // bail must not drop a guard that would then delete that build's overlay.
    write_sync_inputs(&mkosi_local, &args.sync_inputs)?;

    let _mkosi_local_guard = RemoveDirOnDrop {
        dir: mkosi_local.clone(),
    };
    let mkosi_local_extra = mkosi_local.join("mkosi.extra");
    fs_err::create_dir_all(&mkosi_local_extra)?;

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

    // Inject cloud-init user-data into mkosi.local/mkosi.extra seed directory (measured in verity root)
    let seed_dir = PathBuf::from("mkosi/base/mkosi.local/mkosi.extra/var/lib/cloud/seed/nocloud");
    if let Some(ref ci) = args.cloud_init {
        inject_cloud_init(ci, &seed_dir)?;
    }

    // Profiles are applied by mkosi automatically via `--profile=NAME` passed
    // through below. Static profile content (mkosi.conf + mkosi.extra/) lives
    // in `mkosi/base/mkosi.profiles/<NAME>/`. Any host-side prep a profile
    // needs (e.g. pulling a binary from a registry into mkosi.local/) is the
    // operator's responsibility — see `bin/confos-fetch-<NAME>` helpers and
    // `make build-<NAME>` targets that chain prep + build.
    let mut profiles = args.profiles.clone();
    let _profile_dir_guards = stage_profile_dirs(
        Path::new("mkosi/base/mkosi.profiles"),
        &args.profile_dirs,
        &mut profiles,
    )?;
    for profile in &profiles {
        tracing::debug!("profile enabled: {profile}");
    }

    // Phase 1: ensure custom kernel artifact is current
    println!("\n=== Step 1/4: Ensuring custom kernel ===");
    let kernel = kernel_cache::ensure_kernel(
        false,
        args.kernel_config_fragment.clone(),
        args.kernel_builder_package.clone(),
    )?;
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
    let mkosi_initrd = initrd_dir
        .join("mkosi.output/image.cpio.gz")
        .canonicalize()?;

    // Assemble a trusted-DSDT early-cpio and prepend it to mkosi's initrd.
    //
    // The kernel feature CONFIG_ACPI_TABLE_UPGRADE scans the initrd stream
    // from the start for `kernel/firmware/acpi/*.aml` and uses each match to
    // replace the firmware-supplied ACPI table of the same signature. We
    // ship our trusted DSDT this way so the kernel runs OUR AML, not the
    // VMM's — closing the "BadAML" attack surface. The override is invisible
    // to mkosi: we just feed it a concatenated stream as --initrd.
    //
    // Order matters: kernel parses the initrd from the start, so the early
    // (uncompressed) cpio MUST precede the gzipped main cpio.
    let initrd_path = assemble_initrd_with_trusted_dsdt(&output, &mkosi_initrd)?;
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

    // mkosi v27 picks its OutputDirectory by checking for `mkosi.output/`
    // under the config dir: present → write artifacts there; absent → drop
    // them next to `mkosi.conf`. confos's downstream code (and the `image.efi`
    // lookup below) assumes the `mkosi.output/` layout, so create it before
    // mkosi is invoked. Otherwise the build succeeds but the UKI / disk /
    // roothash artifacts land at the wrong path and confos errors out with
    // "UKI .efi not found in mkosi output."
    fs_err::create_dir_all(mkosi_dir.join("mkosi.output"))?;

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
    for profile in &profiles {
        mkosi_args.push(format!("--profile={profile}"));
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

    // Read firmware + UKI bytes once per platform; the file reads aren't
    // free on large OVMF builds. SNP and TDX use DIFFERENT firmware
    // binaries (confos-edk2 with IgvmHobArea for SNP, Ubuntu/TDVF-capable
    // for TDX) so we read each independently.
    let snp_fw_bytes_opt = match snp_firmware.as_ref() {
        Some(fw) => Some(fs_err::read(fw)?),
        None => None,
    };
    let tdx_fw_bytes_opt = match tdx_firmware.as_ref() {
        Some(fw) => Some(fs_err::read(fw)?),
        None => None,
    };
    let uki_bytes = fs_err::read(&output_uki)?;

    // Step 4: Build IGVM variants (optional). Emits one `guest-smp{N}.igvm`
    // per value in `args.smp` (default [2, 4, 8, 16] — the standard
    // powers-of-two), each as its own entry in manifest.snp_variants[].
    // The firmware + UKI bytes are read once and reused; the per-variant
    // cost is just the measurement pass.
    let igvm_variants: Vec<manifest::SnpVariant> = if platform.needs_snp() {
        println!(
            "\n=== Step 4a: Building IGVM variants (smp = {:?}) ===",
            args.smp
        );

        let fw_bytes = snp_fw_bytes_opt
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SNP firmware path required for IGVM build"))?;

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
            let result = igvm::invoke::build_snp(fw_bytes, &uki_bytes, smp)?;

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
            out.push(manifest::SnpVariant {
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
    } else {
        println!(
            "\n=== Step 4a: Skipping IGVM (platform = {:?}) ===",
            platform
        );
        Vec::new()
    };

    // Copy firmware(s) into output so the directory is self-contained
    // for publish/run. SNP firmware lives at `OVMF.fd` (back-compat),
    // TDX firmware at `OVMF.tdx.fd` when present.
    if let Some(ref fw) = snp_firmware {
        let output_fw = output.join("OVMF.fd");
        fs_err::copy(fw, &output_fw)?;
        println!("SNP firmware: {}", output_fw.display());
    }
    if let Some(ref fw) = tdx_firmware {
        let output_fw = output.join("OVMF.tdx.fd");
        fs_err::copy(fw, &output_fw)?;
        println!("TDX firmware: {}", output_fw.display());
    }

    // move raw disk image to output
    let disk_path = output.join("disk.raw");
    let base_abs = base_image.canonicalize()?;
    tools::sudo_mv(&base_abs, &disk_path)?;

    // Step 4b: TDX measurement pass. We need to read the now-user-owned
    // disk image (for the RTMR[1] GPT event), so this runs after the
    // sudo_mv above. The pass is fast (a few hundred ms on a 1G UKI +
    // 4G disk) — no disk crypto, no firmware-side simulation beyond
    // MRTD's MEM.PAGE.ADD / MR.EXTEND replay.
    let tdx_measurement: Option<manifest::TdxMeasurement> = if platform.needs_tdx() {
        println!("\n=== Step 4b: Computing TDX measurements ===");
        let fw_bytes = tdx_fw_bytes_opt
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("TDX firmware path required for TDX measurement"))?;
        let disk_bytes = fs_err::read(&disk_path)?;
        let m =
            tdx_measure::measure_uki_topology_invariant(fw_bytes, &uki_bytes, Some(&disk_bytes))
                .map_err(|e| anyhow::anyhow!("TDX measurement failed: {e}"))?;
        println!("  MRTD:    {}", m.mrtd);
        println!("  RTMR[1]: {}", m.rtmr1);
        println!("  RTMR[2]: {}", m.rtmr2);
        let tdx_fw_entry = tdx_firmware
            .as_ref()
            .map(|fw| -> anyhow::Result<manifest::FileEntry> {
                Ok(manifest::FileEntry {
                    path: "OVMF.tdx.fd".to_string(),
                    sha256: manifest::sha256_file(fw)?,
                })
            })
            .transpose()?;
        Some(manifest::TdxMeasurement {
            mrtd: m.mrtd,
            rtmr1: m.rtmr1,
            rtmr2: m.rtmr2,
            firmware: tdx_fw_entry,
        })
    } else {
        println!(
            "\n=== Step 4b: Skipping TDX measurement (platform = {:?}) ===",
            platform
        );
        None
    };

    println!("\n=== Calculating checksums ===");
    // mkosi records checksums for several split artifacts in this file. Do
    // not rely on their order: image.roothash may precede image.raw.
    let mkosi_checksums = fs_err::read(mkosi_output.join("image.SHA256SUMS"))?;
    let disk_checksum = checksum_for_file(&mkosi_checksums, "image.raw")?;
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
    // build.platform: a short tag for the runner / publisher to know
    // what hardware this artifact was prepared for. The set of accepted
    // values mirrors commands::run::ALLOWED_PLATFORMS — keep these two
    // in sync. A both-platforms build encodes as `multi`; a TDX-only
    // build encodes as `tdx`; SNP-only stays `snp`. This lets a verifier
    // reading just `build.platform` know which entries to expect in
    // `snp_variants[]` / `tdx`. The historical `generic` value remains
    // accepted by `confos run` for back-compat with non-confidential
    // KVM-only builds.
    let platform_tag = match platform {
        BuildPlatform::Snp => "snp".to_string(),
        BuildPlatform::Tdx => "tdx".to_string(),
        BuildPlatform::Both => "multi".to_string(),
    };
    // Write manifest
    let build_manifest = manifest::BuildManifest {
        version: manifest::MANIFEST_VERSION,
        build: manifest::BuildConfig {
            timestamp: chrono_now(),
            memory: args.memory.clone(),
            format: "raw".to_string(),
            platform: platform_tag,
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
            // inputs.firmware records the SNP firmware specifically.
            // Both common firmware files ship as `OVMF.fd`, so recording
            // the source basename would collide with the TDX firmware in
            // a both-platform build. Use the deterministic output-relative
            // path that `Copy firmware(s) into output` writes earlier
            // ("OVMF.fd" for SNP), so a verifier reading the manifest
            // resolves it the same way regardless of where the build
            // pulled its firmware from. The TDX firmware is recorded
            // separately under `tdx.firmware` with path "OVMF.tdx.fd".
            firmware: snp_firmware
                .as_ref()
                .map(|fw| -> anyhow::Result<manifest::FileEntry> {
                    Ok(manifest::FileEntry {
                        path: "OVMF.fd".to_string(),
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
        snp_variants: igvm_variants,
        tdx: tdx_measurement,
    };
    let manifest_path = output.join("manifest.json");
    manifest::write_manifest(&build_manifest, &manifest_path)?;

    println!("\n===============================");
    println!("  Build complete!");
    println!("  Output:     {}", output.display());
    for v in &build_manifest.snp_variants {
        println!("  IGVM:       {} (smp={})", v.igvm.path, v.smp);
    }
    println!("  Disk:       {}", disk_path.display());
    println!("  Manifest:   {}", manifest_path.display());
    println!("  Root hash:  {roothash}");
    for v in &build_manifest.snp_variants {
        println!(
            "  Launch digest (smp={}): {}",
            v.smp, v.measurement.snp_launch_digest
        );
    }
    if let Some(ref t) = build_manifest.tdx {
        println!("  TDX MRTD:    {}", t.mrtd);
        println!("  TDX RTMR[1]: {}", t.rtmr1);
        println!("  TDX RTMR[2]: {}", t.rtmr2);
    }
    if args.cloud_init.is_some() {
        println!("  Cloud-init: measured in verity root");
    }
    println!("===============================");

    Ok(())
}

/// Return the SHA-256 recorded for `filename` in a sha256sum-compatible file.
fn checksum_for_file(contents: &[u8], filename: &str) -> anyhow::Result<String> {
    let contents = std::str::from_utf8(contents)
        .map_err(|e| anyhow::anyhow!("checksum file is not valid UTF-8: {e}"))?;
    let mut found = None;

    for line in contents.lines() {
        let Some((checksum, recorded_name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let recorded_name = recorded_name
            .trim_start()
            .strip_prefix('*')
            .unwrap_or(recorded_name.trim_start());
        if recorded_name != filename {
            continue;
        }
        if checksum.len() != 64 || !checksum.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("invalid SHA-256 for {filename} in checksum file: {checksum:?}");
        }
        if found.is_some() {
            anyhow::bail!("duplicate checksum entries for {filename}");
        }
        found = Some(checksum.to_ascii_lowercase());
    }

    found.ok_or_else(|| anyhow::anyhow!("checksum for {filename} not found in checksum file"))
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
        "instance-id: confos-sealed\nlocal-hostname: confos\n",
    )?;

    println!("Cloud-init: config measured in image, will run at boot");

    Ok(())
}

/// RAII guard that removes a directory this build staged into the repo, after
/// the mkosi run and including when an error path drops the guard early. Two
/// kinds of instance exist: one for the `mkosi.local/` overlay (which holds
/// every per-build file injection — extra, kernel, console, cloud-init, sync
/// inputs — so a single cleanup covers them all), and one per `--profile-dir`
/// copy under `mkosi.profiles/`.
///
/// The guard cannot run on a hard kill, so both staging paths also recover
/// their own leftovers on the next build — see `write_sync_inputs` and
/// `sweep_stale_staged_profiles`.
struct RemoveDirOnDrop {
    dir: PathBuf,
}

impl Drop for RemoveDirOnDrop {
    fn drop(&mut self) {
        let _ = tools::force_remove_dir_all(&self.dir);
    }
}

/// Sits at a staged profile's root (next to mkosi.conf, so never inside the
/// image's mkosi.extra tree) to mark the copy as ours, and to name the build
/// that owns it — see `sweep_stale_staged_profiles`. Its absence is what
/// identifies a genuine in-tree profile, which is never ours to remove.
const STAGED_PROFILE_MARKER: &str = ".confos-staged-profile";

/// Records which `mkosi.local/<NAME>` files the last `--sync-input` run wrote.
/// Without it a hard-killed build's values linger and the *next* build — which
/// may pass no `--sync-input` at all, so it has no name to overwrite — silently
/// builds against them.
const SYNC_INPUT_MANIFEST: &str = ".confos-sync-inputs";

/// Body of a marker/manifest file: the pid of the build that wrote it, so a
/// later build can tell "leftover from a hard kill" from "in use right now".
fn owner_stamp() -> String {
    format!("pid={}\n", std::process::id())
}

/// The pid recorded in a marker/manifest body, if it has one. A file truncated
/// by a kill mid-write has none — callers treat that as "no live owner", which
/// is the state it in fact records.
fn recorded_owner(contents: &str) -> Option<u32> {
    contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid="))
        .and_then(|pid| pid.trim().parse().ok())
}

/// Is the build that wrote a marker still running? `/proc/<pid>` is the
/// cheapest check that needs no signal permission. confos builds are
/// Linux-only (mkosi is), so anywhere else we can only assume it is gone —
/// which keeps recovery working rather than wedging on an unanswerable check.
fn owner_is_live(pid: u32) -> bool {
    cfg!(target_os = "linux") && Path::new("/proc").join(pid.to_string()).exists()
}

/// The pid recorded in `path`, or `None` if the file is missing, unreadable,
/// or carries no pid.
fn owner_of(path: &Path) -> Option<u32> {
    fs_err::read_to_string(path)
        .ok()
        .as_deref()
        .and_then(recorded_owner)
}

/// Charset shared by staged file/profile names: safe as a filesystem path
/// component and as an mkosi `--profile=` value (no separators, no shell
/// metacharacters).
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Split a `--sync-input NAME=VALUE` spec; NAME becomes a file under
/// mkosi.local, so it gets the same charset restriction as profile names.
fn parse_sync_input(spec: &str) -> anyhow::Result<(&str, &str)> {
    let (name, value) = spec
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("--sync-input wants NAME=VALUE, got {spec:?}"))?;
    if !is_safe_name(name) {
        anyhow::bail!("--sync-input name {name:?} must match [A-Za-z0-9_-]+");
    }
    Ok((name, value))
}

/// Clear `--sync-input` files a previous build left in `mkosi.local` and stage
/// this build's, recording the names so the next build can do the same.
///
/// mkosi.local deliberately survives across builds (operator prep stages into
/// it), so a sync input from a hard-killed build would otherwise be picked up
/// by whatever runs next — an image built against, say, the wrong component
/// ref, with nothing in the log to say so. Only names this tool recorded are
/// removed; anything a fetch helper staged is left alone.
fn write_sync_inputs(mkosi_local: &Path, specs: &[String]) -> anyhow::Result<()> {
    // Parse everything before touching the filesystem so a typo in the last
    // spec doesn't leave the earlier ones staged.
    let inputs = specs
        .iter()
        .map(|spec| parse_sync_input(spec))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let manifest = mkosi_local.join(SYNC_INPUT_MANIFEST);
    if let Ok(recorded) = fs_err::read_to_string(&manifest) {
        match recorded_owner(&recorded) {
            // Deleting a concurrent build's inputs mid-run would corrupt it
            // far more quietly than saying so here.
            Some(pid) if owner_is_live(pid) => anyhow::bail!(
                "another confos build (pid {pid}) is using {}; concurrent builds \
                 share mkosi.local and would corrupt each other. Wait for it to \
                 finish, or remove {} if that pid is gone.",
                mkosi_local.display(),
                manifest.display()
            ),
            _ => {
                // is_safe_name re-checked on the way out: the manifest is a
                // file on disk, so it cannot be trusted to hold path
                // components that stay inside mkosi.local.
                for name in recorded.lines().map(str::trim).filter(|n| is_safe_name(n)) {
                    let stale = mkosi_local.join(name);
                    if fs_err::symlink_metadata(&stale).is_ok() {
                        tracing::warn!(
                            "removing stale --sync-input {name} left by an interrupted build"
                        );
                        fs_err::remove_file(&stale)?;
                    }
                }
                fs_err::remove_file(&manifest)?;
            }
        }
    }

    if inputs.is_empty() {
        return Ok(());
    }
    fs_err::create_dir_all(mkosi_local)?;
    // Manifest before content: a kill mid-write must still leave every name
    // recorded, or the next build inherits a leftover it cannot name.
    let names: Vec<&str> = inputs.iter().map(|(name, _)| *name).collect();
    fs_err::write(
        &manifest,
        format!("{}{}\n", owner_stamp(), names.join("\n")),
    )?;
    for (name, value) in inputs {
        fs_err::write(mkosi_local.join(name), format!("{value}\n"))?;
    }
    Ok(())
}

/// Profile name for an out-of-tree profile dir: its basename.
fn external_profile_name(dir: &Path) -> anyhow::Result<String> {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !is_safe_name(name) {
        anyhow::bail!(
            "--profile-dir {} basename is not a valid mkosi profile name (want [A-Za-z0-9_-]+)",
            dir.display()
        );
    }
    Ok(name.to_string())
}

/// Stage each `--profile-dir` as a copy under `mkosi/base/mkosi.profiles/`
/// for the build's duration and add its basename to `profiles` — unless the
/// caller already pinned a position for it via `--profile`, which decides
/// mkosi's config-merge order. Returns the guards that remove the copies.
///
/// A copy, not a symlink or an `Include=`: mkosi v26 resolves profiles only
/// under cwd's `mkosi.profiles/`, and CLI includes parse BEFORE the main
/// config tree while profiles parse after it — only a staged copy keeps
/// profile merge precedence identical to in-tree, which is what lets a
/// moved-out profile build bit-identically.
fn stage_profile_dirs(
    profiles_root: &Path,
    dirs: &[PathBuf],
    profiles: &mut Vec<String>,
) -> anyhow::Result<Vec<RemoveDirOnDrop>> {
    // Unconditionally, before looking at this build's own arguments: an
    // orphan left by a hard kill is what the *next* build trips over, and that
    // build need not pass --profile-dir at all.
    sweep_stale_staged_profiles(profiles_root)?;

    let mut guards: Vec<RemoveDirOnDrop> = Vec::new();
    for dir in dirs {
        let name = external_profile_name(dir)?;
        if !dir.join("mkosi.conf").is_file() {
            anyhow::bail!("--profile-dir has no mkosi.conf: {}", dir.display());
        }
        let target = profiles_root.join(&name);
        if guards.iter().any(|g| g.dir == target) {
            anyhow::bail!(
                "two --profile-dir arguments share the basename {name:?}; the second ({}) would silently replace the first",
                dir.display()
            );
        }
        if fs_err::symlink_metadata(&target).is_ok() {
            // The sweep above already took anything stale, so what is still
            // here is either a genuine in-tree profile or a copy a concurrent
            // build is using. Both are hard errors: deleting the latter would
            // pull the config tree out from under a running mkosi.
            let held = match owner_of(&target.join(STAGED_PROFILE_MARKER)) {
                Some(pid) => format!("staged by a running confos build (pid {pid})"),
                None => "an in-tree profile".to_string(),
            };
            anyhow::bail!(
                "--profile-dir {} collides with existing profile {name:?} at {} ({held})",
                dir.display(),
                target.display()
            );
        }
        reject_escaping_symlinks(dir)?;
        guards.push(RemoveDirOnDrop {
            dir: target.clone(),
        });
        // Marker before content: a kill mid-copy must still leave a
        // recognizably-ours dir, or the next run hard-errors on it.
        fs_err::create_dir_all(&target)?;
        fs_err::write(target.join(STAGED_PROFILE_MARKER), owner_stamp())?;
        copy_extra(dir, &target)?;
        tracing::info!("out-of-tree profile {name} staged from {}", dir.display());
        if !profiles.contains(&name) {
            profiles.push(name);
        }
    }
    Ok(guards)
}

/// Remove staged profiles under `mkosi.profiles/` whose build is gone.
///
/// An orphan is worse than clutter. `--profile <name>` cannot tell a leftover
/// copy from a real in-tree profile, so a later build that never asked for the
/// out-of-tree profile silently gets its content — and since `mkosi.profiles/`
/// is git-tracked territory that `.gitignore` cannot cover by name, `git add
/// -A` would commit another repo's profile into this one.
fn sweep_stale_staged_profiles(profiles_root: &Path) -> anyhow::Result<()> {
    let entries = match fs_err::read_dir(profiles_root) {
        Ok(entries) => entries,
        // No profiles dir at all is normal in tests and harmless in a build:
        // staging creates it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let marker = dir.join(STAGED_PROFILE_MARKER);
        // Unmarked means in-tree: never ours to remove.
        if !marker.is_file() {
            continue;
        }
        if let Some(pid) = owner_of(&marker).filter(|&pid| owner_is_live(pid)) {
            tracing::warn!(
                "leaving staged profile {} alone; confos build pid {pid} is still using it",
                dir.display()
            );
            continue;
        }
        tracing::warn!(
            "removing stale staged profile {} left by an interrupted build",
            dir.display()
        );
        tools::force_remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Profile subtrees whose symlinks are resolved inside the built image rather
/// than on the host, so re-parenting the profile cannot break them.
const IMAGE_RELATIVE_SUBTREES: [&str; 2] = ["mkosi.extra", "mkosi.skeleton"];

/// Reject symlinks in a profile's config tree that point outside the profile.
///
/// Staging re-parents the directory under `mkosi.profiles/<name>/`, so a
/// relative link that escaped its own root — `mkosi.conf.d/shared.conf ->
/// ../../common/shared.conf`, perfectly valid in the consumer's repo — either
/// dangles or, worse, resolves to an unrelated file inside *this* repo. A
/// profile dir has to be self-contained; say so loudly instead of building
/// against whatever the link happens to land on.
fn reject_escaping_symlinks(root: &Path) -> anyhow::Result<()> {
    fn walk(root: &Path, rel: &Path) -> anyhow::Result<()> {
        for entry in fs_err::read_dir(root.join(rel))? {
            let entry = entry?;
            let name = entry.file_name();
            let ft = entry.file_type()?;
            if rel.as_os_str().is_empty()
                && IMAGE_RELATIVE_SUBTREES
                    .iter()
                    .any(|s| name.as_os_str() == *s)
            {
                continue;
            }
            let rel_child = rel.join(&name);
            if ft.is_symlink() {
                let target = fs_err::read_link(entry.path())?;
                if escapes_root(rel, &target) {
                    anyhow::bail!(
                        "--profile-dir {} contains symlink {} -> {}, which points outside \
                         the profile; a profile dir must be self-contained because staging \
                         re-parents it under mkosi.profiles/",
                        root.display(),
                        rel_child.display(),
                        target.display()
                    );
                }
            } else if ft.is_dir() {
                walk(root, &rel_child)?;
            }
        }
        Ok(())
    }
    walk(root, Path::new(""))
}

/// Would a symlink at `rel_parent/<link>` with target `target` resolve outside
/// the tree root? Purely lexical, and deliberately conservative: intermediate
/// components are counted as directories, so a link that only *looks* like it
/// escapes is rejected too.
fn escapes_root(rel_parent: &Path, target: &Path) -> bool {
    use std::path::Component;
    let mut depth = rel_parent.components().count() as isize;
    for component in target.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return true,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
        }
    }
    false
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

// Per-profile fetchers live in bin/confos-fetch-<NAME> shell scripts; the
// `make build-<NAME>` Makefile targets chain fetch + build. Keeping this Rust
// code unaware of registries and pinned digests means the confos CLI stays
// focused on the image-build pipeline.

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Compile the trusted DSDT (ASL → AML), build a one-file early cpio
/// containing `kernel/firmware/acpi/dsdt.aml`, and prepend it to the
/// mkosi-built initrd. Returns the path to the combined initrd, which is
/// what the rest of the pipeline (UKI assembly, RTMR[2] measurement,
/// IGVM launch digest) sees as "the initrd."
///
/// The kernel parses the initrd stream in order from offset 0. An
/// uncompressed newc cpio at the start is recognized and consumed, then
/// the gzipped cpio that follows is decompressed and unpacked normally —
/// any file path appearing in BOTH is overwritten by the later (main)
/// cpio. That's fine for us: we only ship one path (`dsdt.aml`) and the
/// main initrd never contains it, so there's no conflict.
fn assemble_initrd_with_trusted_dsdt(
    output: &Path,
    mkosi_initrd: &Path,
) -> anyhow::Result<PathBuf> {
    let dsdt_asl = PathBuf::from("mkosi/base/acpi-tables/dsdt.asl");
    if !dsdt_asl.exists() {
        anyhow::bail!("trusted DSDT not found at {}", dsdt_asl.display());
    }

    // iasl writes both the .aml and a disassembly listing next to its -p
    // argument. Put it in the per-build output directory so a parallel
    // build can't race on a shared temp path.
    let dsdt_aml = output.join("dsdt.aml");
    if dsdt_aml.exists() {
        fs_err::remove_file(&dsdt_aml)?;
    }
    let dsdt_aml_str = dsdt_aml.to_string_lossy().into_owned();
    let dsdt_asl_str = dsdt_asl.to_string_lossy().into_owned();
    tools::run_command_streaming("iasl", &["-p", &dsdt_aml_str, &dsdt_asl_str])
        .map_err(|e| anyhow::anyhow!("iasl failed compiling {}: {}", dsdt_asl.display(), e))?;
    if !dsdt_aml.exists() {
        anyhow::bail!(
            "iasl reported success but {} is missing",
            dsdt_aml.display()
        );
    }

    // Stage the AML in the path layout CONFIG_ACPI_TABLE_UPGRADE expects:
    //   kernel/firmware/acpi/<table>.aml
    // built inside a fresh dir so the cpio archive contains only this entry
    // (no stray dotfiles or sibling artifacts).
    let staging = output.join(".early-acpi");
    if staging.exists() {
        fs_err::remove_dir_all(&staging)?;
    }
    let staged_dir = staging.join("kernel/firmware/acpi");
    fs_err::create_dir_all(&staged_dir)?;
    fs_err::copy(&dsdt_aml, staged_dir.join("dsdt.aml"))?;

    // Build the early cpio. GNU cpio reads file paths on stdin; we list
    // entries relative to the staging dir and run cpio with cwd at that
    // dir so the archive holds relative paths. Use newc format (the only
    // format the kernel's CONFIG_INITRAMFS_COMPRESSION supports).
    let early_cpio = output.join("early.cpio");
    build_early_cpio(&staging, &early_cpio)?;

    // Concatenate early.cpio || mkosi_initrd. The combined file is what
    // mkosi receives via --initrd and what RTMR[2] / launch digests
    // ultimately measure as `.initrd`.
    let combined = output.join("combined-initrd.img");
    concat_files(&[&early_cpio, mkosi_initrd], &combined)?;

    // The mkosi initrd's gzip header carries the compression wall-clock
    // time — the one non-deterministic input to every downstream
    // measurement. Patch it in the combined copy (mkosi's own output is
    // root-owned) so consecutive builds are bit-identical.
    let early_cpio_len = fs_err::metadata(&early_cpio)?.len();
    zero_gzip_mtime(&combined, early_cpio_len)?;

    // Staging tree and intermediate cpio are throwaway once concatenation
    // succeeds; leaving them around would just clutter the output dir.
    fs_err::remove_dir_all(&staging)?;
    fs_err::remove_file(&early_cpio)?;

    combined.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "canonicalizing combined initrd {}: {}",
            combined.display(),
            e
        )
    })
}

/// Build a newc-format cpio archive from every regular file and
/// directory under `root` (descending), writing the archive to `out`.
///
/// Uses GNU cpio in -o (copy-out) mode reading null-terminated paths on
/// stdin. Cwd is set to `root` so paths inside the archive are relative,
/// matching what the kernel's initramfs unpacker expects.
fn build_early_cpio(root: &Path, out: &Path) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    let root_abs = root.canonicalize()?;
    let out_abs = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };

    // INVARIANT: This cpio must be byte-reproducible across builds. The
    // cpio bytes are concatenated into the initrd and feed into RTMR[2]
    // (TDX) and the SNP launch digest. Two clean checkouts of the same
    // commit must produce identical cpio bytes — otherwise the manifest's
    // measurements drift between builds and verifiers can't pin a
    // reference.
    //
    // Three sources of non-determinism in `find | cpio -o -H newc` that we
    // have to neutralize:
    //   1. Directory enumeration order. find walks readdir order, which is
    //      filesystem-dependent (ext4 htree, tmpfs, btrfs, etc.). Pipe
    //      through `sort -z` so the path list is byte-sorted.
    //   2. File mtime. newc cpio headers embed mtime per entry. We can't
    //      rely on touch(1) idempotency in CI, so the easiest fix is to
    //      hint GNU cpio with SOURCE_DATE_EPOCH=0 (its --reproducible
    //      flag is too new to require on every host).
    //   3. uid/gid embedded in headers. We pass --owner=root:root so the
    //      cpio is built with the same identity regardless of who runs
    //      the build.
    let mut find = Command::new("find")
        .arg(".")
        .arg("-mindepth")
        .arg("1")
        .arg("-print0")
        .current_dir(&root_abs)
        .stdout(Stdio::piped())
        .spawn()?;
    let find_stdout = find
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("could not capture find stdout"))?;

    // LC_ALL=C forces byte-wise sort. glibc's default locale collation
    // can reorder Unicode filenames (and even some single-byte
    // characters depending on UCA tailoring), which would silently
    // drift RTMR[2] / SNP launch digest between hosts with different
    // locale settings. ASCII-only filenames today, but lock the order
    // down so it stays stable if a future contributor adds an
    // SSDT-from-some-vendor file with non-ASCII bytes.
    let mut sort = Command::new("sort")
        .arg("-z")
        .env("LC_ALL", "C")
        .stdin(Stdio::from(find_stdout))
        .stdout(Stdio::piped())
        .spawn()?;
    let sort_stdout = sort
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("could not capture sort stdout"))?;

    // Zero mtimes recursively before cpio reads them. GNU cpio's
    // --reproducible flag only zeros device/inode numbers; mtime in the
    // newc header still comes from st_mtime. Walking the tree and forcing
    // mtime to 0 (epoch) is the only way to get bit-identical headers
    // across builds.
    zero_mtimes(&root_abs)?;

    let cpio_out = std::fs::File::create(&out_abs)?;
    let cpio = Command::new("cpio")
        .args([
            "-o",
            "-H",
            "newc",
            "--null",
            "--quiet",
            "--owner=+0:+0",
            "--reproducible",
        ])
        .current_dir(&root_abs)
        .stdin(Stdio::from(sort_stdout))
        .stdout(Stdio::from(cpio_out))
        .stderr(Stdio::inherit())
        .spawn()?;
    let cpio_output = cpio.wait_with_output()?;
    let find_status = find.wait()?;
    let sort_status = sort.wait()?;
    if !find_status.success() {
        anyhow::bail!(
            "find failed enumerating {} (exit {:?})",
            root_abs.display(),
            find_status.code()
        );
    }
    if !sort_status.success() {
        anyhow::bail!(
            "sort failed sorting cpio input (exit {:?})",
            sort_status.code()
        );
    }
    if !cpio_output.status.success() {
        anyhow::bail!(
            "cpio failed building {} (exit {:?})",
            out_abs.display(),
            cpio_output.status.code()
        );
    }
    Ok(())
}

/// Recursively reset access and modification times on every entry under
/// `root` to the Unix epoch (0). Used to neutralize per-file mtime as a
/// source of cpio newc header non-determinism — see the comment in
/// `build_early_cpio` for context.
fn zero_mtimes(root: &Path) -> anyhow::Result<()> {
    let epoch = filetime::FileTime::from_unix_time(0, 0);
    fn walk(p: &Path, epoch: filetime::FileTime) -> std::io::Result<()> {
        let md = std::fs::symlink_metadata(p)?;
        // symlink times can't be set portably; the parent's lstat carries
        // the canonical timestamp for cpio's view of the symlink anyway.
        if !md.file_type().is_symlink() {
            filetime::set_file_times(p, epoch, epoch)?;
        }
        if md.is_dir() {
            for entry in std::fs::read_dir(p)? {
                walk(&entry?.path(), epoch)?;
            }
        }
        Ok(())
    }
    walk(root, epoch).map_err(Into::into)
}

/// Zero the MTIME field of the gzip member that starts at `offset` in `path`.
///
/// mkosi's `CompressOutput=gzip` stamps the compression wall-clock time into
/// bytes 4..8 of the gzip header (`SourceDateEpoch=0` does not reach gzip,
/// which has no SOURCE_DATE_EPOCH support). Those four bytes are the only
/// non-deterministic bytes in the combined initrd, and they cascade into the
/// UKI, the disk image, every SNP launch digest, and RTMR[1]/RTMR[2] — so
/// consecutive builds of identical content would publish different reference
/// measurements. MTIME=0 is defined by RFC 1952 as "no timestamp available";
/// the kernel's initramfs unpacker never reads it.
fn zero_gzip_mtime(path: &Path, offset: u64) -> anyhow::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = fs_err::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;
    // ID1=0x1f ID2=0x8b (gzip magic), CM=8 (deflate) per RFC 1952
    if header[0..3] != [0x1f, 0x8b, 0x08] {
        anyhow::bail!(
            "expected a gzip (deflate) member at offset {} of {}, found {:02x?} — \
             cannot normalize initrd MTIME for reproducible builds",
            offset,
            path.display(),
            &header[0..3],
        );
    }
    file.seek(SeekFrom::Start(offset + 4))?;
    file.write_all(&[0u8; 4])?;
    Ok(())
}

/// Concatenate the byte streams of `parts` (in order) into `out`.
fn concat_files(parts: &[&Path], out: &Path) -> anyhow::Result<()> {
    let mut sink = fs_err::File::create(out)?;
    for p in parts {
        let mut src = fs_err::File::open(p)?;
        std::io::copy(&mut src, &mut sink)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    const ROOT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DISK_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn checksum_for_file_selects_image_raw_by_name() {
        let sums = format!("{ROOT_HASH}  image.roothash\n{DISK_HASH}  image.raw\n");
        assert_eq!(
            checksum_for_file(sums.as_bytes(), "image.raw").unwrap(),
            DISK_HASH
        );
    }

    #[test]
    fn checksum_for_file_accepts_binary_marker() {
        let sums = format!("{DISK_HASH} *image.raw\n");
        assert_eq!(
            checksum_for_file(sums.as_bytes(), "image.raw").unwrap(),
            DISK_HASH
        );
    }

    #[test]
    fn checksum_for_file_rejects_missing_image_raw() {
        let sums = format!("{ROOT_HASH}  image.roothash\n");
        assert!(checksum_for_file(sums.as_bytes(), "image.raw")
            .unwrap_err()
            .to_string()
            .contains("image.raw not found"));
    }

    #[test]
    fn checksum_for_file_rejects_invalid_image_raw_hash() {
        assert!(checksum_for_file(b"not-a-hash  image.raw\n", "image.raw")
            .unwrap_err()
            .to_string()
            .contains("invalid SHA-256 for image.raw"));
    }

    #[test]
    fn checksum_for_file_rejects_duplicate_image_raw_entries() {
        let sums = format!("{DISK_HASH}  image.raw\n{ROOT_HASH}  image.raw\n");
        assert!(checksum_for_file(sums.as_bytes(), "image.raw")
            .unwrap_err()
            .to_string()
            .contains("duplicate checksum entries"));
    }

    fn gzip_with_mtime(payload: &[u8], mtime: u32) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::GzBuilder::new()
            .mtime(mtime)
            .write(Vec::new(), flate2::Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn zero_gzip_mtime_zeroes_only_the_mtime_field() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("combined.img");
        // gzip member preceded by other bytes, as in the combined initrd
        // (early cpio || gzipped mkosi initrd)
        let prefix = b"EARLY-CPIO-BYTES";
        let gz = gzip_with_mtime(b"initrd payload", 0x6a54_4bad);
        let mut original = prefix.to_vec();
        original.extend_from_slice(&gz);
        fs_err::write(&path, &original).unwrap();

        zero_gzip_mtime(&path, prefix.len() as u64).unwrap();

        let patched = fs_err::read(&path).unwrap();
        let off = prefix.len();
        let mut expected = original.clone();
        expected[off + 4..off + 8].fill(0);
        assert_eq!(patched, expected);
        // the member must still decompress to the same payload
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&patched[off..]), &mut out)
            .unwrap();
        assert_eq!(out, b"initrd payload");
    }

    #[test]
    fn zero_gzip_mtime_makes_time_shifted_archives_identical() {
        // The regression this guards: identical payloads compressed at
        // different wall-clock times produce different bytes until the
        // MTIME field is normalized.
        let a = gzip_with_mtime(b"same initrd", 0x6a54_4bad);
        let b = gzip_with_mtime(b"same initrd", 0x6a54_4c30);
        assert_ne!(a, b);

        let dir = TempDir::new().unwrap();
        let pa = dir.path().join("a");
        let pb = dir.path().join("b");
        fs_err::write(&pa, &a).unwrap();
        fs_err::write(&pb, &b).unwrap();
        zero_gzip_mtime(&pa, 0).unwrap();
        zero_gzip_mtime(&pb, 0).unwrap();

        assert_eq!(fs_err::read(&pa).unwrap(), fs_err::read(&pb).unwrap());
    }

    #[test]
    fn zero_gzip_mtime_rejects_non_gzip_data() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("not-gzip");
        fs_err::write(&path, b"070701-plain-cpio-not-gzip").unwrap();
        assert!(zero_gzip_mtime(&path, 0).is_err());
    }

    fn mk_profile(dir: &Path) {
        fs_err::create_dir_all(dir).unwrap();
        fs_err::write(dir.join("mkosi.conf"), "[Content]\n").unwrap();
    }

    #[test]
    fn stage_profile_dirs_stages_marked_copy_and_appends_name() {
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let src = src_root.path().join("c8s");
        mk_profile(&src);
        fs_err::write(src.join("data"), b"x").unwrap();
        let mut profiles = vec!["gpu".to_string()];
        let guards = stage_profile_dirs(profiles_root.path(), &[src], &mut profiles).unwrap();
        let staged = profiles_root.path().join("c8s");
        assert!(staged.join("mkosi.conf").is_file());
        assert!(staged.join("data").is_file());
        assert!(staged.join(STAGED_PROFILE_MARKER).is_file());
        assert_eq!(profiles, ["gpu", "c8s"]);
        drop(guards);
        assert!(!staged.exists());
    }

    #[test]
    fn stage_profile_dirs_keeps_explicit_profile_position() {
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let src = src_root.path().join("c8s");
        mk_profile(&src);
        let mut profiles = vec!["c8s".to_string(), "dev".to_string()];
        let _g = stage_profile_dirs(profiles_root.path(), &[src], &mut profiles).unwrap();
        assert_eq!(profiles, ["c8s", "dev"]);
    }

    #[test]
    fn stage_profile_dirs_rejects_unmarked_collision_but_replaces_stale() {
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let src = src_root.path().join("c8s");
        mk_profile(&src);
        let in_tree = profiles_root.path().join("c8s");
        mk_profile(&in_tree);
        let mut profiles = vec![];
        assert!(
            stage_profile_dirs(
                profiles_root.path(),
                std::slice::from_ref(&src),
                &mut profiles
            )
            .is_err(),
            "unmarked (in-tree) profile must be a hard error"
        );
        fs_err::write(in_tree.join(STAGED_PROFILE_MARKER), b"").unwrap();
        fs_err::write(in_tree.join("stale"), b"").unwrap();
        let _g = stage_profile_dirs(profiles_root.path(), &[src], &mut profiles).unwrap();
        assert!(
            !profiles_root.path().join("c8s/stale").exists(),
            "marked leftover must be replaced, not merged"
        );
    }

    #[test]
    fn stage_profile_dirs_rejects_duplicate_basenames() {
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let a = src_root.path().join("a/c8s");
        let b = src_root.path().join("b/c8s");
        mk_profile(&a);
        mk_profile(&b);
        let mut profiles = vec![];
        assert!(stage_profile_dirs(profiles_root.path(), &[a, b], &mut profiles).is_err());
    }

    /// A dead pid: /proc/0 never exists, so this always reads as "owner gone".
    const DEAD_PID: &str = "pid=0\n";

    #[test]
    fn stage_profile_dirs_sweeps_orphans_even_without_profile_dir_args() {
        // The build that trips over an orphan is typically the one that never
        // asked for an out-of-tree profile at all.
        let profiles_root = TempDir::new().unwrap();
        let orphan = profiles_root.path().join("c8s");
        mk_profile(&orphan);
        fs_err::write(orphan.join(STAGED_PROFILE_MARKER), DEAD_PID).unwrap();
        let in_tree = profiles_root.path().join("attest");
        mk_profile(&in_tree);

        let mut profiles = vec!["attest".to_string()];
        let _g = stage_profile_dirs(profiles_root.path(), &[], &mut profiles).unwrap();

        assert!(!orphan.exists(), "orphaned staged profile must be swept");
        assert!(in_tree.exists(), "in-tree profile must survive the sweep");
    }

    #[test]
    fn stage_profile_dirs_spares_and_rejects_a_live_owners_copy() {
        // Deleting a concurrent build's staged copy would break it silently;
        // before the marker existed this collision was a hard error, and it
        // has to stay one.
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let src = src_root.path().join("c8s");
        mk_profile(&src);
        let live = profiles_root.path().join("c8s");
        mk_profile(&live);
        fs_err::write(
            live.join(STAGED_PROFILE_MARKER),
            format!("pid={}\n", std::process::id()),
        )
        .unwrap();

        let mut profiles = vec![];
        let Err(err) = stage_profile_dirs(profiles_root.path(), &[src], &mut profiles) else {
            panic!("collision with a live owner's copy must be an error");
        };

        assert!(live.exists(), "a live build's staged copy must survive");
        assert!(
            err.to_string().contains("running confos build"),
            "error should name the live owner, got: {err}"
        );
    }

    #[test]
    fn stage_profile_dirs_rejects_escaping_symlink_in_profile_config() {
        let src_root = TempDir::new().unwrap();
        let profiles_root = TempDir::new().unwrap();
        let src = src_root.path().join("c8s");
        mk_profile(&src);
        fs_err::create_dir_all(src.join("mkosi.conf.d")).unwrap();
        std::os::unix::fs::symlink(
            "../../common/shared.conf",
            src.join("mkosi.conf.d/shared.conf"),
        )
        .unwrap();

        let mut profiles = vec![];
        assert!(
            stage_profile_dirs(profiles_root.path(), &[src], &mut profiles).is_err(),
            "a config symlink escaping the profile root must not be staged"
        );
        assert!(
            !profiles_root.path().join("c8s").exists(),
            "the rejected profile must not be left half-staged"
        );
    }

    #[test]
    fn reject_escaping_symlinks_allows_self_contained_and_image_relative_links() {
        let root = TempDir::new().unwrap();
        let root = root.path();
        fs_err::create_dir_all(root.join("mkosi.conf.d")).unwrap();
        fs_err::write(root.join("mkosi.conf.d/base.conf"), b"").unwrap();
        // Stays inside the profile: fine, staging moves it wholesale.
        std::os::unix::fs::symlink("base.conf", root.join("mkosi.conf.d/alias.conf")).unwrap();
        std::os::unix::fs::symlink("mkosi.conf.d/base.conf", root.join("top.conf")).unwrap();
        // Resolved in the built image, not on the host — exempt even when
        // absolute or escaping.
        fs_err::create_dir_all(root.join("mkosi.extra/usr/bin")).unwrap();
        std::os::unix::fs::symlink(
            "/usr/lib/systemd/systemd",
            root.join("mkosi.extra/usr/bin/init"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "../../../../etc/hosts",
            root.join("mkosi.extra/usr/bin/hosts"),
        )
        .unwrap();

        reject_escaping_symlinks(root).unwrap();
    }

    #[test]
    fn reject_escaping_symlinks_rejects_absolute_host_links() {
        let root = TempDir::new().unwrap();
        std::os::unix::fs::symlink("/etc/confos/shared.conf", root.path().join("mkosi.conf"))
            .unwrap();
        assert!(reject_escaping_symlinks(root.path()).is_err());
    }

    #[test]
    fn escapes_root_counts_depth_lexically() {
        let at_root = Path::new("");
        let nested = Path::new("mkosi.conf.d");
        assert!(!escapes_root(at_root, Path::new("sibling.conf")));
        assert!(!escapes_root(at_root, Path::new("./a/b")));
        assert!(!escapes_root(nested, Path::new("../mkosi.conf")));
        assert!(!escapes_root(nested, Path::new("../a/../mkosi.conf")));
        assert!(escapes_root(at_root, Path::new("../outside")));
        assert!(escapes_root(nested, Path::new("../../outside")));
        assert!(escapes_root(nested, Path::new("/abs/path")));
    }

    #[test]
    fn recorded_owner_reads_the_pid_or_nothing() {
        assert_eq!(recorded_owner("pid=1234\n"), Some(1234));
        assert_eq!(recorded_owner("pid=7\nc8s-ref\nother\n"), Some(7));
        // Truncated by a kill mid-write, or a marker from an older confos.
        assert_eq!(recorded_owner(""), None);
        assert_eq!(recorded_owner("c8s-ref\n"), None);
        assert_eq!(recorded_owner("pid=notanumber\n"), None);
    }

    #[test]
    fn write_sync_inputs_stages_values_and_records_names() {
        let local = TempDir::new().unwrap();
        write_sync_inputs(local.path(), &["c8s-ref=abc123".to_string()]).unwrap();

        assert_eq!(
            fs_err::read_to_string(local.path().join("c8s-ref")).unwrap(),
            "abc123\n"
        );
        let manifest = fs_err::read_to_string(local.path().join(SYNC_INPUT_MANIFEST)).unwrap();
        assert_eq!(recorded_owner(&manifest), Some(std::process::id()));
        assert!(manifest.contains("c8s-ref"));
    }

    #[test]
    fn write_sync_inputs_clears_leftovers_even_with_no_inputs_of_its_own() {
        // The failure this guards: a killed build's ref is silently consumed
        // by the next build, which passed no --sync-input at all.
        let local = TempDir::new().unwrap();
        fs_err::write(local.path().join("c8s-ref"), "stale-ref\n").unwrap();
        fs_err::write(
            local.path().join(SYNC_INPUT_MANIFEST),
            format!("{DEAD_PID}c8s-ref\n"),
        )
        .unwrap();

        write_sync_inputs(local.path(), &[]).unwrap();

        assert!(!local.path().join("c8s-ref").exists());
        assert!(!local.path().join(SYNC_INPUT_MANIFEST).exists());
    }

    #[test]
    fn write_sync_inputs_leaves_operator_prep_alone() {
        // bin/confos-fetch-<NAME> stages into mkosi.local before the build;
        // only names we recorded are ours to delete.
        let local = TempDir::new().unwrap();
        fs_err::write(local.path().join("attest-binary"), b"prep").unwrap();
        fs_err::write(
            local.path().join(SYNC_INPUT_MANIFEST),
            format!("{DEAD_PID}c8s-ref\n"),
        )
        .unwrap();

        write_sync_inputs(local.path(), &[]).unwrap();

        assert!(local.path().join("attest-binary").exists());
    }

    #[test]
    fn write_sync_inputs_refuses_to_disturb_a_live_build() {
        let local = TempDir::new().unwrap();
        fs_err::write(local.path().join("c8s-ref"), "in-use\n").unwrap();
        fs_err::write(
            local.path().join(SYNC_INPUT_MANIFEST),
            format!("pid={}\nc8s-ref\n", std::process::id()),
        )
        .unwrap();

        assert!(write_sync_inputs(local.path(), &[]).is_err());
        assert_eq!(
            fs_err::read_to_string(local.path().join("c8s-ref")).unwrap(),
            "in-use\n"
        );
    }

    #[test]
    fn write_sync_inputs_rejects_bad_spec_before_staging_anything() {
        let local = TempDir::new().unwrap();
        assert!(write_sync_inputs(
            local.path(),
            &["good=1".to_string(), "bad spec".to_string()]
        )
        .is_err());
        assert!(!local.path().join("good").exists());
    }

    #[test]
    fn parse_sync_input_splits_on_first_eq() {
        assert_eq!(
            parse_sync_input("c8s-ref=3a2517b").unwrap(),
            ("c8s-ref", "3a2517b")
        );
        assert_eq!(parse_sync_input("k=a=b").unwrap(), ("k", "a=b"));
        assert_eq!(parse_sync_input("empty=").unwrap(), ("empty", ""));
    }

    #[test]
    fn parse_sync_input_rejects_bad_specs() {
        for bad in ["no-eq", "=value", "a/b=x", "a b=x"] {
            assert!(
                parse_sync_input(bad).is_err(),
                "expected rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn external_profile_name_takes_basename() {
        assert_eq!(
            external_profile_name(Path::new("../c8s/node-guest-image/c8s")).unwrap(),
            "c8s"
        );
        assert_eq!(
            external_profile_name(Path::new("/abs/path/my_profile-2")).unwrap(),
            "my_profile-2"
        );
    }

    #[test]
    fn external_profile_name_rejects_unsafe_names() {
        for bad in ["/", "..", "a b", "pr!file", ""] {
            assert!(
                external_profile_name(Path::new(bad)).is_err(),
                "expected rejection for {bad:?}"
            );
        }
    }

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
            let _guard = RemoveDirOnDrop { dir: dir.clone() };
            assert!(dir.exists());
        }
        assert!(!dir.exists());
    }

    #[test]
    fn mkosi_local_cleanup_swallows_missing_directory() {
        let parent = TempDir::new().unwrap();
        let dir = parent.path().join("never-existed");
        drop(RemoveDirOnDrop { dir });
        // No panic == pass.
    }

    // Shells out to GNU cpio/sort flags that BSD userland (macOS) rejects.
    // confos build runs on Linux only (mkosi is Linux-only).
    #[cfg(target_os = "linux")]
    #[test]
    fn build_early_cpio_is_reproducible_across_mtime_and_enumeration_order() {
        // The cpio bytes feed into RTMR[2] / SNP launch digest, so they
        // must be byte-stable across builds. Two sources of drift we
        // explicitly defend against: (a) file mtime, (b) readdir-order
        // dependence on enumeration. This test exercises both.
        use std::os::unix::fs::OpenOptionsExt;

        let build = |mtimes: &[u64], create_order: &[&str]| -> Vec<u8> {
            let src = TempDir::new().unwrap();
            // Create the same logical content but in a different order so
            // any readdir-order bug surfaces. Use different mtimes per
            // build so any mtime leak surfaces.
            for &name in create_order {
                let p = src.path().join("kernel/firmware/acpi").join(name);
                fs_err::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o644)
                    .open(&p)
                    .unwrap();
                fs_err::write(&p, b"payload").unwrap();
            }
            // Set distinct mtimes per file (and per build) — these should
            // be erased by zero_mtimes() before the cpio runs.
            for (i, name) in create_order.iter().enumerate() {
                let p = src.path().join("kernel/firmware/acpi").join(name);
                let t = filetime::FileTime::from_unix_time(mtimes[i] as i64, 0);
                filetime::set_file_times(&p, t, t).unwrap();
            }
            let out_dir = TempDir::new().unwrap();
            let cpio_path = out_dir.path().join("early.cpio");
            build_early_cpio(src.path(), &cpio_path).unwrap();
            fs_err::read(&cpio_path).unwrap()
        };

        let a = build(&[1_700_000_000, 1_700_000_001], &["a.aml", "b.aml"]);
        let b = build(&[1_750_000_000, 1_750_000_002], &["b.aml", "a.aml"]);
        assert_eq!(
            a, b,
            "cpio bytes drifted across mtime / enumeration order; this breaks RTMR[2] / SNP launch digest reproducibility"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn build_early_cpio_packs_files_from_root() {
        // Sanity: build_early_cpio reads a directory and produces a
        // non-empty newc cpio whose magic ("070701") appears at the start
        // of the first entry header. This is what
        // CONFIG_ACPI_TABLE_UPGRADE scans for at offset 0 of the initrd.
        let src = TempDir::new().unwrap();
        let nested = src.path().join("kernel/firmware/acpi");
        fs_err::create_dir_all(&nested).unwrap();
        fs_err::write(nested.join("dsdt.aml"), b"DSDT-fake-aml").unwrap();

        let out_dir = TempDir::new().unwrap();
        let cpio_path = out_dir.path().join("early.cpio");
        build_early_cpio(src.path(), &cpio_path).unwrap();

        let bytes = fs_err::read(&cpio_path).unwrap();
        assert!(!bytes.is_empty(), "cpio archive should not be empty");
        assert!(
            bytes.starts_with(b"070701"),
            "cpio archive should start with newc magic '070701', got {:?}",
            &bytes[..6.min(bytes.len())]
        );
        // The aml file's bytes should appear verbatim somewhere in the
        // archive (newc stores file data inline after each header).
        assert!(
            bytes
                .windows(b"DSDT-fake-aml".len())
                .any(|w| w == b"DSDT-fake-aml"),
            "cpio archive should embed the staged file data"
        );
    }

    #[test]
    fn concat_files_preserves_order_and_bytes() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let out = dir.path().join("out");
        fs_err::write(&a, b"AAA").unwrap();
        fs_err::write(&b, b"BBB").unwrap();
        concat_files(&[a.as_path(), b.as_path()], &out).unwrap();
        assert_eq!(fs_err::read(&out).unwrap(), b"AAABBB");
    }
}
