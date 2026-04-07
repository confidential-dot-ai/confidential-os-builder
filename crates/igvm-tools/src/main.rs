// This module is CLI glue. It parses arguments, calls library functions,
// and writes output. No complex logic belongs here.
//
// NOTE: This tool targets QEMU+KVM exclusively. The IGVM construction,
// page ordering, measurement algorithm (batch flushing), and VMSA overrides
// all replicate QEMU+KVM's specific behavior. The computed launch digest
// will match hardware attestation only when the guest runs on QEMU+KVM.

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use igvm_tools::builder::Builder;
use igvm_tools::hob::{IgvmDataList, IgvmDataType};
use igvm_tools::manifest::{
    sha256_hex, BuildConfig, FileInfo, InputFiles, Manifest, MeasurementInfo,
};
use igvm_tools::measure;
use igvm_tools::ovmfmeta::{OvmfMeta, OvmfRegionType};
use igvm_tools::x86regs::{flat32_mode_regs, real_mode_regs_at};

#[derive(Parser)]
#[command(
    name = "igvm-tools",
    version,
    about = "Build and measure IGVM files for AMD SEV-SNP confidential VMs (QEMU+KVM)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build an IGVM file from firmware, kernel, and optional components
    Build(Box<BuildArgs>),
    /// Measure an existing IGVM file and print the SNP launch digest
    Measure(MeasureArgs),
}

#[derive(Parser)]
struct BuildArgs {
    /// OVMF firmware image
    #[arg(long, value_name = "FILE")]
    firmware: String,

    /// UEFI variable store (optional)
    #[arg(long, value_name = "FILE")]
    vars: Option<String>,

    /// Kernel EFI binary (optional)
    #[arg(long, value_name = "FILE")]
    kernel: Option<String>,

    /// Shim EFI binary (optional)
    #[arg(long, value_name = "FILE")]
    shim: Option<String>,

    /// Secure boot PK certificate (.auth file, optional)
    #[arg(long, value_name = "FILE")]
    pk: Option<String>,

    /// Secure boot KEK certificate (.auth file, optional)
    #[arg(long, value_name = "FILE")]
    kek: Option<String>,

    /// Secure boot db certificate (.auth file, optional)
    #[arg(long, value_name = "FILE")]
    db: Option<String>,

    /// Secure boot dbx revocation list (.auth file, optional)
    #[arg(long, value_name = "FILE")]
    dbx: Option<String>,

    /// Platform type
    #[arg(long, default_value = "snp")]
    platform: Platform,

    /// Boot mode
    #[arg(long, default_value = "real16")]
    boot_mode: BootMode,

    /// Number of vCPUs
    #[arg(long, default_value = "1")]
    smp: u32,

    /// Output IGVM file
    #[arg(short, long, value_name = "FILE")]
    output: String,

    /// Output JSON manifest (optional)
    #[arg(long, value_name = "FILE")]
    manifest: Option<String>,

    /// Print OVMF metadata before building
    #[arg(long)]
    meta: bool,

    /// Verbose measurement output
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Parser)]
struct MeasureArgs {
    /// IGVM file to measure
    igvm_file: String,

    /// Verbose measurement output
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum Platform {
    Snp,
    Native,
    #[value(name = "snp+native")]
    SnpNative,
}

#[derive(Clone, Debug, ValueEnum)]
enum BootMode {
    Real16,
    Flat32,
}

fn read_optional(path: &Option<String>, label: &str) -> Result<Option<Vec<u8>>, String> {
    path.as_ref()
        .map(|p| std::fs::read(p).map_err(|e| format!("read {label} {p}: {e}")))
        .transpose()
}

struct HobInputs {
    kernel: Option<Vec<u8>>,
    shim: Option<Vec<u8>>,
    pk: Option<Vec<u8>>,
    kek: Option<Vec<u8>>,
    db: Option<Vec<u8>>,
    dbx: Option<Vec<u8>>,
}

impl HobInputs {
    fn has_data(&self) -> bool {
        self.kernel.is_some()
            || self.shim.is_some()
            || self.pk.is_some()
            || self.kek.is_some()
            || self.db.is_some()
            || self.dbx.is_some()
    }
}

fn add_hob_data(
    builder: &mut Builder,
    ovmfmeta: &Option<OvmfMeta>,
    inputs: &HobInputs,
) -> Result<(), String> {
    if !inputs.has_data() {
        return Ok(());
    }

    let hobarea = ovmfmeta
        .as_ref()
        .and_then(|m| {
            m.regions
                .iter()
                .find(|r| r.etype == OvmfRegionType::IgvmHobArea)
        })
        .ok_or("OVMF firmware has no IgvmHobArea region (needed for kernel/shim/cert injection)")?;

    let mut hoblist = IgvmDataList::new(0x20000000); // start at 512 MB

    // Order matters: GPA is assigned sequentially by add() order, so changing
    // the order of these calls changes blob placement and the launch digest.

    // Secure boot certs (measured)
    if let Some(ref blob) = inputs.pk {
        hoblist.add(blob, IgvmDataType::Pk, true);
    }
    if let Some(ref blob) = inputs.kek {
        hoblist.add(blob, IgvmDataType::Kek, true);
    }
    if let Some(ref blob) = inputs.db {
        hoblist.add(blob, IgvmDataType::Db, true);
    }
    if let Some(ref blob) = inputs.dbx {
        hoblist.add(blob, IgvmDataType::Dbx, true);
    }

    // Shim (unmeasured — verified via secure boot at runtime)
    if let Some(ref blob) = inputs.shim {
        hoblist.add(blob, IgvmDataType::Shim, false);
    }

    // Kernel (measured — included in launch digest)
    if let Some(ref blob) = inputs.kernel {
        hoblist.add(blob, IgvmDataType::Kernel, true);
    }

    // Place HOB index in the HOB area
    let hobs_blob = hoblist.hobs();
    builder.remove_page_data_in_range(hobarea.memory.0, hobarea.memory.1);
    builder.add_data_pages(hobarea.memory.0, &hobs_blob);

    // Place data blobs (measured first, then unmeasured)
    for (addr, blob) in hoblist.blobs(true) {
        builder.remove_page_data_in_range(addr, blob.len());
        builder.add_data_pages(addr, blob);
    }
    for (addr, blob) in hoblist.blobs(false) {
        builder.remove_page_data_in_range(addr, blob.len());
        builder.add_data_pages_unmeasured(addr, blob);
    }

    eprintln!("Added {} HOB entries to IGVM", hoblist.entry_count());
    Ok(())
}

fn write_manifest(
    args: &BuildArgs,
    igvm_blob: &[u8],
    firmware: &[u8],
    vars: &Option<Vec<u8>>,
    kernel: &Option<Vec<u8>>,
    shim: &Option<Vec<u8>>,
    result: &measure::MeasureResult,
) -> Result<(), String> {
    let Some(ref manifest_path) = args.manifest else {
        return Ok(());
    };

    let manifest = Manifest {
        version: 1,
        igvm_file: args.output.clone(),
        igvm_sha256: sha256_hex(igvm_blob),
        measurement: MeasurementInfo {
            snp_launch_digest: hex::encode(result.launch_digest),
            algorithm: "sha384".to_string(),
            page_count: result.page_count,
            vmsa_count: result.vmsa_count,
        },
        config: BuildConfig {
            platform: format!("{:?}", args.platform).to_lowercase(),
            boot_mode: format!("{:?}", args.boot_mode).to_lowercase(),
            smp: args.smp,
        },
        inputs: InputFiles {
            firmware: FileInfo {
                path: args.firmware.clone(),
                sha256: sha256_hex(firmware),
            },
            vars: vars.as_ref().map(|v| FileInfo {
                path: args.vars.as_ref().expect("vars path set").clone(),
                sha256: sha256_hex(v),
            }),
            kernel: kernel.as_ref().map(|k| FileInfo {
                path: args.kernel.as_ref().expect("kernel path set").clone(),
                sha256: sha256_hex(k),
            }),
            shim: shim.as_ref().map(|s| FileInfo {
                path: args.shim.as_ref().expect("shim path set").clone(),
                sha256: sha256_hex(s),
            }),
        },
        generated_at: chrono::Utc::now().to_rfc3339(),
    };

    let json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("serialize manifest: {e}"))?;
    std::fs::write(manifest_path, &json).map_err(|e| format!("write {manifest_path}: {e}"))?;
    eprintln!("Wrote {manifest_path}");
    Ok(())
}

fn do_build(args: &BuildArgs) -> Result<(), String> {
    // Phase 1: Read inputs
    let firmware = std::fs::read(&args.firmware)
        .map_err(|e| format!("read firmware {}: {e}", args.firmware))?;
    let vars_blob = read_optional(&args.vars, "vars")?;
    let kernel_blob = read_optional(&args.kernel, "kernel")?;
    let shim_blob = read_optional(&args.shim, "shim")?;

    // Phase 2: Parse OVMF metadata
    let ovmfmeta = OvmfMeta::new(&firmware);

    if args.meta {
        if let Some(ref meta) = ovmfmeta {
            meta.print();
        } else {
            eprintln!("warning: no OVMF metadata found in firmware");
        }
    }

    // Phase 3: Build IGVM
    let mut builder = Builder::new();
    let use_snp = matches!(args.platform, Platform::Snp | Platform::SnpNative);
    let use_native = matches!(args.platform, Platform::Native | Platform::SnpNative);

    if use_native {
        builder.add_native_platform();
        if matches!(args.boot_mode, BootMode::Flat32) {
            builder.add_native_context(&flat32_mode_regs(None));
        }
    }

    if use_snp {
        builder.add_snp_platform();

        let bsp_regs = match args.boot_mode {
            BootMode::Flat32 => flat32_mode_regs(None),
            BootMode::Real16 => real_mode_regs_at(0xFFFFFFF0),
        };
        builder.add_snp_vmsa_context(&bsp_regs, false, 0);

        if args.smp > 1 {
            let ap_reset_addr = ovmfmeta.as_ref().and_then(|m| m.sev_reset_addr).ok_or(
                "OVMF firmware does not contain SEV-ES reset address (needed for --smp > 1)",
            )?;
            let ap_regs = real_mode_regs_at(ap_reset_addr);
            for vp in 1..args.smp {
                let vp_index = u16::try_from(vp).map_err(|_| "vCPU index exceeds u16 range")?;
                builder.add_snp_vmsa_context(&ap_regs, false, vp_index);
            }
        }

        if let Some(ref meta) = ovmfmeta {
            builder.add_ovmf_snp_pages(meta);
        }
        builder.add_snp_policy(None);
    }

    if let Some(ref meta) = ovmfmeta {
        builder.add_ovmf_igvm_params(meta);
    }

    if matches!(args.boot_mode, BootMode::Real16) {
        builder.add_firmware_1m(&firmware);
    }
    builder.add_firmware_4g(&firmware);

    if let Some(ref vars) = vars_blob {
        builder.add_uefivars(vars, firmware.len());
    }

    // Phase 3b: Kernel/shim/cert HOB injection
    let hob_inputs = HobInputs {
        kernel: kernel_blob.clone(),
        shim: shim_blob.clone(),
        pk: read_optional(&args.pk, "pk")?,
        kek: read_optional(&args.kek, "kek")?,
        db: read_optional(&args.db, "db")?,
        dbx: read_optional(&args.dbx, "dbx")?,
    };

    add_hob_data(&mut builder, &ovmfmeta, &hob_inputs)?;

    // Phase 4: Finalize and measure
    let igvm = builder
        .finalize()
        .map_err(|e| format!("finalize igvm: {e}"))?;

    let result = measure::measure_snp(&igvm, args.verbose)?;

    eprintln!("SNP launch digest: {}", hex::encode(result.launch_digest));
    eprintln!("Pages: {}, VMSAs: {}", result.page_count, result.vmsa_count);

    // Phase 5: Serialize and write
    let mut igvm_blob = Vec::new();
    igvm.serialize(&mut igvm_blob)
        .map_err(|e| format!("serialize igvm: {e}"))?;

    std::fs::write(&args.output, &igvm_blob).map_err(|e| format!("write {}: {e}", args.output))?;
    eprintln!("Wrote {}", args.output);

    write_manifest(
        args,
        &igvm_blob,
        &firmware,
        &vars_blob,
        &kernel_blob,
        &shim_blob,
        &result,
    )?;

    // Print digest to stdout for piping
    println!("{}", hex::encode(result.launch_digest));
    Ok(())
}

fn do_measure(args: &MeasureArgs) -> Result<(), String> {
    let blob =
        std::fs::read(&args.igvm_file).map_err(|e| format!("read {}: {e}", args.igvm_file))?;
    let igvm =
        igvm::IgvmFile::new_from_binary(&blob, None).map_err(|e| format!("parse igvm: {e}"))?;

    let result = measure::measure_snp(&igvm, args.verbose)?;

    eprintln!("Pages: {}, VMSAs: {}", result.page_count, result.vmsa_count);
    println!("{}", hex::encode(result.launch_digest));

    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::Build(ref args) => do_build(args),
        Commands::Measure(args) => do_measure(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
