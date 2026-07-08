use std::fs;
use std::path::{Path, PathBuf};
use std::str;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use tdx_measure::{ccel, pe, rtmr, tdvf};

#[derive(Parser)]
#[command(name = "tdx-measure")]
#[command(about = "Offline TDX measurement computation and attestation verification")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Which boot model to measure. Both are OVMF/TDVF firmware; they differ in
/// how the kernel is loaded and therefore in the RTMR[1]/RTMR[2] event chain.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BootMode {
    /// steep UKI boot: TDVF -> systemd-boot -> Unified Kernel Image.
    Uki,
    /// TDVF direct kernel boot (`-kernel`/`-append`), e.g. kata-qemu-tdx.
    /// Not kata-specific — any direct-booted guest under TDVF measures this way.
    DirectKernel,
}

#[derive(Subcommand)]
enum Commands {
    /// Compute expected TDX measurements for a UKI or direct-kernel boot
    Measure {
        /// Boot model to measure (default: uki)
        #[arg(long, value_enum, default_value_t = BootMode::Uki)]
        boot_mode: BootMode,

        /// Path to the UKI EFI binary (required for --boot-mode uki)
        #[arg(long)]
        uki: Option<PathBuf>,

        /// Path to the kernel PE / bzImage (required for --boot-mode direct-kernel)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// File containing the exact kernel command line, i.e. qemu's `-append`
        /// value (required for --boot-mode direct-kernel)
        #[arg(long)]
        cmdline_file: Option<PathBuf>,

        /// Path to the OVMF/TDVF firmware binary
        #[arg(long)]
        firmware: Option<PathBuf>,

        /// Path to the GPT disk image containing the UKI (for RTMR[1] GPT event)
        #[arg(long)]
        disk: Option<PathBuf>,

        /// VM memory size (e.g. "2G", "4096M")
        #[arg(long, default_value = "2G")]
        memory: String,

        /// CCEL from a reference boot with same firmware+config.
        /// Extracts both ACPI hashes and boot variables — the easiest
        /// way to get a full RTMR[0] match.
        #[arg(long)]
        platform_ccel: Option<PathBuf>,

        /// Path to pre-computed ACPI hash file (from extract-platform)
        #[arg(long)]
        acpi_hashes: Option<PathBuf>,

        /// Path to ACPI tables blob (acpi_tables.bin from extract.sh)
        #[arg(long)]
        acpi_tables: Option<PathBuf>,

        /// Path to ACPI RSDP blob (rsdp.bin from extract.sh)
        #[arg(long)]
        acpi_rsdp: Option<PathBuf>,

        /// Path to ACPI table loader blob (table_loader.bin from extract.sh)
        #[arg(long)]
        acpi_loader: Option<PathBuf>,

        /// Path to directory containing boot variable files (BootOrder.bin, Boot0000.bin, etc.)
        #[arg(long)]
        boot_vars: Option<PathBuf>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Verify TDX measurements against a CCEL event log and TDREPORT
    Verify {
        /// Path to CCEL event log binary
        #[arg(long)]
        ccel: PathBuf,

        /// Path to TDREPORT binary (1024 bytes)
        #[arg(long)]
        tdreport: PathBuf,

        /// Path to UKI file (to verify RTMR[2] digests)
        #[arg(long)]
        uki: Option<PathBuf>,
    },

    /// Inspect a CCEL event log (human-readable)
    Inspect {
        /// Path to CCEL event log binary
        #[arg(long)]
        ccel: PathBuf,

        /// Path to TDREPORT binary (optional, for comparison)
        #[arg(long)]
        tdreport: Option<PathBuf>,
    },

    /// Extract platform data (ACPI hashes + boot vars) from a reference CCEL.
    /// Run once per firmware+config, reuse for all UKI measurements.
    ExtractPlatform {
        /// Path to CCEL event log binary from a reference boot
        #[arg(long)]
        ccel: PathBuf,

        /// Output directory for extracted files
        #[arg(long, default_value = ".")]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Measure {
            boot_mode,
            uki,
            kernel,
            cmdline_file,
            firmware,
            disk,
            memory,
            platform_ccel,
            acpi_hashes,
            acpi_tables,
            acpi_rsdp,
            acpi_loader,
            boot_vars,
            json,
        } => cmd_measure(
            boot_mode,
            uki.as_deref(),
            kernel.as_deref(),
            cmdline_file.as_deref(),
            firmware.as_deref(),
            disk.as_deref(),
            &memory,
            platform_ccel.as_deref(),
            acpi_hashes.as_deref(),
            acpi_tables.as_deref(),
            acpi_rsdp.as_deref(),
            acpi_loader.as_deref(),
            boot_vars.as_deref(),
            json,
        ),
        Commands::Verify {
            ccel,
            tdreport,
            uki,
        } => cmd_verify(&ccel, &tdreport, uki.as_deref()),
        Commands::Inspect { ccel, tdreport } => cmd_inspect(&ccel, tdreport.as_deref()),
        Commands::ExtractPlatform { ccel, output } => cmd_extract_platform(&ccel, &output),
    }
}

/// Measure a TDVF **direct kernel boot** (`-kernel`/`-append`), e.g. the guest
/// launched by `kata-qemu-tdx`. Only MRTD, RTMR[1] and RTMR[2] are verifiable;
/// RTMR[0] encodes ACPI/memory/vCPU config and is intentionally not checked,
/// and RTMR[3] is left for a runtime workload measurement.
fn cmd_measure_direct_kernel(
    kernel_path: Option<&Path>,
    cmdline_file: Option<&Path>,
    firmware_path: Option<&Path>,
    json: bool,
) -> Result<()> {
    let kernel_path =
        kernel_path.context("--kernel is required for --boot-mode direct-kernel")?;
    let cmdline_path =
        cmdline_file.context("--cmdline-file is required for --boot-mode direct-kernel")?;

    let kernel = fs::read(kernel_path)
        .with_context(|| format!("Failed to read kernel: {}", kernel_path.display()))?;
    let cmdline_raw = fs::read_to_string(cmdline_path)
        .with_context(|| format!("Failed to read cmdline: {}", cmdline_path.display()))?;
    // qemu's `-append` is a single line; drop a trailing newline the file may
    // carry (the measured LoadOptions do not include it).
    let cmdline = cmdline_raw.strip_suffix('\n').unwrap_or(&cmdline_raw);

    let rtmr1 = rtmr::compute_rtmr1_direct_kernel(&kernel)?;
    let rtmr2 = rtmr::compute_rtmr2_direct_kernel(cmdline);

    let mrtd = match firmware_path {
        Some(fw) => {
            let fw_data = fs::read(fw)
                .with_context(|| format!("Failed to read firmware: {}", fw.display()))?;
            let t = tdvf::Tdvf::parse(&fw_data).context("Failed to parse TDVF metadata")?;
            Some(t.mrtd().context("Failed to compute MRTD")?)
        }
        None => None,
    };

    if json {
        let mut result = serde_json::Map::new();
        result.insert(
            "boot_mode".into(),
            serde_json::Value::String("direct-kernel".into()),
        );
        if let Some(ref m) = mrtd {
            result.insert("mrtd".into(), serde_json::Value::String(hex::encode(m)));
        }
        result.insert("rtmr1".into(), serde_json::Value::String(hex::encode(&rtmr1)));
        result.insert("rtmr2".into(), serde_json::Value::String(hex::encode(&rtmr2)));
        result.insert(
            "rtmr3".into(),
            serde_json::Value::String(hex::encode([0u8; 48])),
        );
        result.insert(
            "rtmr0_note".into(),
            serde_json::Value::String(
                "not computed: RTMR[0] encodes ACPI/memory/vCPU config and is not verified".into(),
            ),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(result))?
        );
    } else {
        eprintln!("Boot model: direct-kernel (TDVF -kernel/-append)");
        eprintln!("Kernel:  {} ({} bytes)", kernel_path.display(), kernel.len());
        eprintln!("Cmdline: {} chars\n", cmdline.len());

        println!("=== TDX Measurements (direct-kernel) ===\n");
        match mrtd {
            Some(ref m) => println!("MRTD:    {}", hex::encode(m)),
            None => println!("MRTD:    (provide --firmware to compute)"),
        }
        println!("RTMR[0]: (not verified -- encodes ACPI/memory/vCPU config)");
        println!("RTMR[1]: {}", hex::encode(&rtmr1));
        println!("RTMR[2]: {}", hex::encode(&rtmr2));
        println!("RTMR[3]: {}", hex::encode([0u8; 48]));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_measure(
    boot_mode: BootMode,
    uki_path: Option<&Path>,
    kernel_path: Option<&Path>,
    cmdline_file: Option<&Path>,
    firmware_path: Option<&Path>,
    disk_path: Option<&Path>,
    memory: &str,
    platform_ccel_path: Option<&Path>,
    acpi_hashes_path: Option<&Path>,
    acpi_tables_path: Option<&Path>,
    acpi_rsdp_path: Option<&Path>,
    acpi_loader_path: Option<&Path>,
    boot_vars_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    if boot_mode == BootMode::DirectKernel {
        return cmd_measure_direct_kernel(kernel_path, cmdline_file, firmware_path, json);
    }

    let uki_path = uki_path.context("--uki is required for --boot-mode uki")?;
    let uki_data = fs::read(uki_path)
        .with_context(|| format!("Failed to read UKI: {}", uki_path.display()))?;

    // Parse UKI PE sections
    let sections = pe::parse_sections(&uki_data).context("Failed to parse UKI PE")?;

    if !json {
        eprintln!("UKI: {} ({} bytes)", uki_path.display(), uki_data.len());
        eprintln!("Sections:");
        for (name, data) in &sections {
            eprintln!("  {:<12} {:>10} bytes", name, data.len());
        }
        eprintln!();
    }

    // Read disk image if provided (for GPT event hash)
    let disk_data = if let Some(dp) = disk_path {
        Some(fs::read(dp).with_context(|| format!("Failed to read disk: {}", dp.display()))?)
    } else {
        None
    };

    // Compute RTMR[1]: UKI PE Authenticode hash + GPT + kernel + boot service constants
    let rtmr1 = rtmr::compute_rtmr1_uki(&uki_data, &sections, disk_data.as_deref())?;

    // Compute RTMR[2]: UKI section measurements
    let (rtmr2_partial, computed_count) = rtmr::compute_rtmr2_uki(&sections)?;

    // Parse firmware once (used for both MRTD and RTMR[0])
    let fw_data = firmware_path
        .map(|fp| {
            fs::read(fp)
                .with_context(|| format!("Failed to read firmware: {}", fp.display()))
        })
        .transpose()?;
    let tdvf_meta = fw_data
        .as_ref()
        .map(|data| tdvf::Tdvf::parse(data).context("Failed to parse TDVF metadata"))
        .transpose()?;

    let mrtd = tdvf_meta
        .as_ref()
        .map(|t| t.mrtd())
        .transpose()?;

    // Load platform data from CCEL, or individual sources.
    // --platform-ccel provides both ACPI hashes and boot variables.
    let platform_ccel_data = if let Some(p) = platform_ccel_path {
        Some(fs::read(p).with_context(|| format!("Failed to read CCEL: {}", p.display()))?)
    } else {
        None
    };

    // Resolve ACPI hashes: --platform-ccel > --acpi-hashes > --acpi-tables/rsdp/loader
    let acpi_hashes = if let Some(ref ccel_data) = platform_ccel_data {
        let h = tdvf::AcpiHashes::extract_from_ccel(ccel_data)
            .context("Failed to extract ACPI hashes from CCEL")?;
        if !json {
            eprintln!("ACPI hashes: extracted from platform CCEL");
        }
        Some(h)
    } else if let Some(hp) = acpi_hashes_path {
        let h = tdvf::AcpiHashes::load(hp).context("Failed to load ACPI hashes")?;
        if !json {
            eprintln!("ACPI hashes: loaded from {}", hp.display());
        }
        Some(h)
    } else {
        match (acpi_tables_path, acpi_rsdp_path, acpi_loader_path) {
            (Some(tables), Some(rsdp), Some(loader)) => {
                let h = tdvf::AcpiHashes::from_files(tables, rsdp, loader)
                    .context("Failed to load ACPI files")?;
                if !json {
                    eprintln!("ACPI hashes: computed from raw blobs");
                }
                Some(h)
            }
            (None, None, None) => None,
            _ => {
                anyhow::bail!(
                    "ACPI table measurement requires all three files: \
                     --acpi-tables, --acpi-rsdp, and --acpi-loader"
                );
            }
        }
    };

    // Resolve boot variables: --platform-ccel > --boot-vars
    let boot_vars = if let Some(ref ccel_data) = platform_ccel_data {
        let bv = tdvf::BootVars::extract_from_ccel(ccel_data)
            .context("Failed to extract boot variables from CCEL")?;
        if !json {
            eprintln!("Boot vars: {} entries from platform CCEL", bv.entries.len());
        }
        Some(bv)
    } else if let Some(dir) = boot_vars_dir {
        let bv = tdvf::BootVars::load_from_dir(dir)
            .context("Failed to load boot variables")?;
        if !json {
            eprintln!("Boot vars: {} entries from {}", bv.entries.len(), dir.display());
        }
        Some(bv)
    } else {
        None
    };

    if !json && (acpi_hashes.is_some() || boot_vars.is_some()) {
        eprintln!();
    }

    // Compute RTMR[0] if firmware provided
    let (rtmr0, rtmr0_events) = if let Some(ref t) = tdvf_meta {
        let mem_size = parse_memory_size(memory)?;
        let (r, count) = t.rtmr0(mem_size, acpi_hashes.as_ref(), boot_vars.as_ref())?;
        (Some(r), count)
    } else {
        (None, 0)
    };

    let has_acpi = acpi_hashes.is_some();
    let has_boot_vars = boot_vars.is_some();

    if json {
        let mut result = serde_json::Map::new();
        if let Some(ref m) = mrtd {
            result.insert("mrtd".into(), serde_json::Value::String(hex::encode(m)));
        }
        if let Some(ref r) = rtmr0 {
            result.insert("rtmr0".into(), serde_json::Value::String(hex::encode(r)));
            result.insert(
                "rtmr0_events".into(),
                serde_json::Value::String(format!("{}/15", rtmr0_events)),
            );
            let mut missing = Vec::new();
            if !has_acpi { missing.push("ACPI tables (--acpi-*)"); }
            if !has_boot_vars { missing.push("boot variables (--boot-vars or --boot-vars-ccel)"); }
            if !missing.is_empty() {
                result.insert(
                    "rtmr0_note".into(),
                    serde_json::Value::String(format!("Partial: excludes {}", missing.join(", "))),
                );
            }
        }
        result.insert(
            "rtmr1".into(),
            serde_json::Value::String(hex::encode(&rtmr1)),
        );
        result.insert(
            "rtmr2".into(),
            serde_json::Value::String(hex::encode(&rtmr2_partial)),
        );
        result.insert(
            "rtmr2_events".into(),
            serde_json::Value::String(format!("{}/14", computed_count)),
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(result))?
        );
    } else {
        println!("=== TDX Measurements ===\n");

        if let Some(ref m) = mrtd {
            println!("MRTD:    {}", hex::encode(m));
        } else {
            println!("MRTD:    (provide --firmware to compute)");
        }

        if let Some(ref r) = rtmr0 {
            println!("RTMR[0]: {} ({}/15 events)", hex::encode(r), rtmr0_events);
            if !has_acpi || !has_boot_vars {
                let mut missing = Vec::new();
                if !has_acpi { missing.push("ACPI tables"); }
                if !has_boot_vars { missing.push("boot variables"); }
                println!("         (partial: excludes {})", missing.join(", "));
            }
        } else {
            println!("RTMR[0]: (provide --firmware to compute)");
        }

        println!("RTMR[1]: {}", hex::encode(&rtmr1));
        if disk_path.is_none() {
            println!("         (without GPT event -- provide --disk for exact value)");
        }
        println!("RTMR[2]: {} ({}/14 events)", hex::encode(&rtmr2_partial), computed_count);
        println!("RTMR[3]: {}", hex::encode([0u8; 48]));
    }

    Ok(())
}

fn cmd_verify(
    ccel_path: &Path,
    tdreport_path: &Path,
    uki_path: Option<&Path>,
) -> Result<()> {
    let ccel_data = fs::read(ccel_path)
        .with_context(|| format!("Failed to read CCEL: {}", ccel_path.display()))?;
    let report_data = fs::read(tdreport_path)
        .with_context(|| format!("Failed to read TDREPORT: {}", tdreport_path.display()))?;

    // IMPORTANT: This verification checks CCEL-to-TDREPORT consistency only.
    // The TDREPORT itself is NOT cryptographically verified here. For full
    // attestation, the TDREPORT must be wrapped in a TDX Quote and verified
    // via Intel's attestation infrastructure.
    eprintln!("WARNING: TDREPORT is not cryptographically verified.");
    eprintln!("         This checks CCEL replay consistency, not TDREPORT authenticity.\n");

    // Parse CCEL events
    let events = ccel::parse_ccel(&ccel_data)?;
    println!("Parsed {} events from CCEL\n", events.len());

    // Replay RTMRs
    let replayed = ccel::replay_rtmrs(&events);
    let hw_rtmrs = ccel::extract_rtmrs_from_tdreport(&report_data)?;

    println!("{:=<90}", "");
    println!(
        "{:<10} | {:>6} | {:>5} | {:<32}... | {:<32}...",
        "Register", "Events", "Match", "Computed", "Hardware"
    );
    println!("{:=<90}", "");

    let names = ["RTMR[0]", "RTMR[1]", "RTMR[2]", "RTMR[3]"];
    let mut all_match = true;
    for i in 0..4 {
        let computed = &replayed[i];
        let hardware = &hw_rtmrs[i];
        let matched = ccel::digests_equal(computed, hardware);
        if !matched {
            all_match = false;
        }
        let event_count = events
            .iter()
            .filter(|e| e.mr_index == (i as u32 + 1))
            .count();
        println!(
            "{:<10} | {:>6} | {:>5} | {}... | {}...",
            names[i],
            event_count,
            if matched { "YES" } else { "NO" },
            &hex::encode(computed)[..32],
            &hex::encode(hardware)[..32],
        );
    }
    println!("{:=<90}", "");

    if all_match {
        println!("\nAll RTMRs match! Event log replay verification PASSED.");
    } else {
        println!("\nWARNING: Some RTMRs do not match!");
        for i in 0..4 {
            if !ccel::digests_equal(&replayed[i], &hw_rtmrs[i]) {
                println!("\n{} MISMATCH:", names[i]);
                println!("  Computed: {}", hex::encode(&replayed[i]));
                println!("  Hardware: {}", hex::encode(&hw_rtmrs[i]));
            }
        }
    }

    // If UKI provided, verify RTMR[2] event digests
    if let Some(uki_p) = uki_path {
        println!("\n{:=<70}", "");
        println!("RTMR[2] UKI verification");
        println!("{:=<70}\n", "");

        let uki_data = fs::read(uki_p)?;
        let sections = pe::parse_sections(&uki_data)?;
        let precomputed = rtmr::precompute_rtmr2_digests(&sections)?;

        let ccel_rtmr2: Vec<&ccel::CcelEvent> =
            events.iter().filter(|e| e.mr_index == 3).collect();

        let mut ok_count = 0;
        for (i, (kind, name, digest)) in precomputed.iter().enumerate() {
            if i < ccel_rtmr2.len() {
                let matched = ccel::digests_equal(digest.as_slice(), ccel_rtmr2[i].sha384_digest.as_slice());
                if matched {
                    ok_count += 1;
                }
                println!(
                    "  Event {:2}: {:<10} {:<8} [{}]",
                    i + 1,
                    name,
                    kind,
                    if matched { "OK" } else { "FAIL" }
                );
            } else {
                println!(
                    "  Event {:2}: {:<10} {:<8} [no CCEL event]",
                    i + 1,
                    name,
                    kind
                );
            }
        }

        if ccel_rtmr2.len() > precomputed.len() {
            println!(
                "\n  {} remaining CCEL events (runtime):",
                ccel_rtmr2.len() - precomputed.len()
            );
            for i in precomputed.len()..ccel_rtmr2.len() {
                println!(
                    "    Event {:2}: digest={}...",
                    i + 1,
                    &hex::encode(&ccel_rtmr2[i].sha384_digest)[..32]
                );
            }
        }

        println!(
            "\n  {}/{} pre-computed digests match CCEL events.",
            ok_count,
            precomputed.len()
        );
    }

    if !all_match {
        bail!("RTMR verification failed: one or more registers do not match");
    }

    Ok(())
}

fn cmd_inspect(
    ccel_path: &Path,
    tdreport_path: Option<&Path>,
) -> Result<()> {
    let ccel_data = fs::read(ccel_path)
        .with_context(|| format!("Failed to read CCEL: {}", ccel_path.display()))?;

    let events = ccel::parse_ccel(&ccel_data)?;

    let mr_name = |idx: u32| -> &str {
        match idx {
            1 => "RTMR[0]",
            2 => "RTMR[1]",
            3 => "RTMR[2]",
            4 => "RTMR[3]",
            _ => "???",
        }
    };

    println!("CCEL Event Log: {} events\n", events.len());

    for (i, ev) in events.iter().enumerate() {
        let etype_name = ccel::event_type_name(ev.event_type);
        let data_preview = if ev.event_data.len() <= 64 {
            match str::from_utf8(&ev.event_data) {
                Ok(s) if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') => {
                    format!("\"{}\"", s.trim_end_matches('\0'))
                }
                _ => format!("{} bytes", ev.event_data.len()),
            }
        } else {
            format!("{} bytes", ev.event_data.len())
        };

        println!(
            "  Event {:3} | {} | {:<32} | digest={}... | {}",
            i + 1,
            mr_name(ev.mr_index),
            etype_name,
            &hex::encode(&ev.sha384_digest)[..24],
            data_preview,
        );
    }

    // Summary
    println!();
    for idx in 1..=4 {
        let count = events.iter().filter(|e| e.mr_index == idx).count();
        if count > 0 {
            println!("  {}: {} events", mr_name(idx), count);
        }
    }

    // If TDREPORT provided, replay and compare
    if let Some(tr_path) = tdreport_path {
        let report_data = fs::read(tr_path)?;
        let replayed = ccel::replay_rtmrs(&events);
        let hw_rtmrs = ccel::extract_rtmrs_from_tdreport(&report_data)?;

        println!("\nReplay verification:");
        for i in 0..4 {
            let matched = ccel::digests_equal(&replayed[i], &hw_rtmrs[i]);
            let count = events
                .iter()
                .filter(|e| e.mr_index == (i as u32 + 1))
                .count();
            if count > 0 || hw_rtmrs[i].iter().any(|&b| b != 0) {
                println!(
                    "  {}: {} events -- {}",
                    mr_name(i as u32 + 1),
                    count,
                    if matched { "MATCH" } else { "MISMATCH" }
                );
            }
        }
    }

    Ok(())
}

fn cmd_extract_platform(ccel_path: &Path, output_dir: &Path) -> Result<()> {
    let ccel_data = fs::read(ccel_path)
        .with_context(|| format!("Failed to read CCEL: {}", ccel_path.display()))?;

    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;

    // Extract ACPI hashes
    let acpi = tdvf::AcpiHashes::extract_from_ccel(&ccel_data)
        .context("Failed to extract ACPI hashes from CCEL")?;
    let acpi_path = output_dir.join("acpi_hashes.txt");
    acpi.save(&acpi_path)?;
    println!("ACPI hashes:");
    println!("  loader: {}", hex::encode(&acpi.loader_hash));
    println!("  rsdp:   {}", hex::encode(&acpi.rsdp_hash));
    println!("  tables: {}", hex::encode(&acpi.tables_hash));
    println!("  -> {}", acpi_path.display());

    // Extract boot variables
    let events = ccel::parse_ccel(&ccel_data)?;
    let mut boot_var_count = 0;
    let boot_dir = output_dir.join("boot-vars");
    fs::create_dir_all(&boot_dir)?;

    for ev in &events {
        if ev.event_type != 0x80000002 || ev.mr_index != 1 {
            continue;
        }
        if let Some((var_name, raw_data)) = ccel::parse_uefi_variable_data(&ev.event_data) {
            let path = boot_dir.join(format!("{}.bin", var_name));
            fs::write(&path, raw_data)?;
            println!("  {} ({} bytes)", var_name, raw_data.len());
            boot_var_count += 1;
        }
    }
    println!("  -> {} ({} variables)", boot_dir.display(), boot_var_count);

    println!("\nUsage:");
    println!("  tdx-measure measure --uki <uki.efi> --firmware <OVMF.fd> --disk <disk.img> \\");
    println!("    --platform-ccel {}  # or use extracted files:", ccel_path.display());
    println!("    --acpi-hashes {} --boot-vars {}", acpi_path.display(), boot_dir.display());

    Ok(())
}

fn parse_memory_size(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty memory size");
    }
    let len = s.len();
    let last = s.chars().last().context("Empty memory size")?;
    let (num_part, multiplier) = match last {
        'k' | 'K' => (&s[..len - 1], 1024u64),
        'm' | 'M' => (&s[..len - 1], 1024u64 * 1024),
        'g' | 'G' => (&s[..len - 1], 1024u64 * 1024 * 1024),
        'T' | 't' => (&s[..len - 1], 1024u64 * 1024 * 1024 * 1024),
        '0'..='9' => (s, 1u64),
        _ => anyhow::bail!("Unknown memory size suffix in '{}'", s),
    };
    let num: u64 = num_part.parse().context("Invalid memory size number")?;
    num.checked_mul(multiplier).context("Memory size overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_size_gigabytes() {
        assert_eq!(parse_memory_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_memory_size("4g").unwrap(), 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_size_megabytes() {
        assert_eq!(parse_memory_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_memory_size("4096m").unwrap(), 4096 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_size_kilobytes() {
        assert_eq!(parse_memory_size("1024K").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory_size("256k").unwrap(), 256 * 1024);
    }

    #[test]
    fn test_parse_memory_size_terabytes() {
        assert_eq!(parse_memory_size("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_size_bare_bytes() {
        assert_eq!(parse_memory_size("1073741824").unwrap(), 1073741824);
    }

    #[test]
    fn test_parse_memory_size_with_whitespace() {
        assert_eq!(parse_memory_size("  2G  ").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_memory_size_empty() {
        assert!(parse_memory_size("").is_err());
        assert!(parse_memory_size("  ").is_err());
    }

    #[test]
    fn test_parse_memory_size_invalid_suffix() {
        assert!(parse_memory_size("2X").is_err());
    }

    #[test]
    fn test_parse_memory_size_invalid_number() {
        assert!(parse_memory_size("abcG").is_err());
    }

    #[test]
    fn test_parse_memory_size_overflow() {
        // u64::MAX / 1024^4 would overflow
        assert!(parse_memory_size("99999999999T").is_err());
    }
}
