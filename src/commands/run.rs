use crate::convert;
use crate::manifest;
use crate::qemu::QemuArgs;
use crate::RunArgs;

pub fn run(args: &RunArgs) -> anyhow::Result<()> {
    tracing::info!(dir = %args.dir.display(), "launching CVM");

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

    // Step 3: Find IGVM file
    let igvm_path = args.dir.join("guest.igvm");
    if !igvm_path.exists() {
        anyhow::bail!("guest.igvm not found in {}", args.dir.display());
    }

    // Step 4: Find disk image using format from manifest
    let disk_path = args.dir.join(format!("disk.{}", manifest.build.format));
    if !disk_path.exists() {
        anyhow::bail!(
            "disk.{} not found in {}",
            manifest.build.format,
            args.dir.display()
        );
    }

    // Step 5: Determine qemu disk format
    let format_enum = match manifest.build.format.as_str() {
        "qcow2" => crate::ImageFormat::Qcow2,
        "vhd" => crate::ImageFormat::Vhd,
        "raw" => crate::ImageFormat::Raw,
        other => anyhow::bail!("unknown disk format in manifest: {other}"),
    };
    let qemu_format = convert::qemu_img_format(&format_enum);

    // Step 6: Launch QEMU
    let qemu_args = QemuArgs {
        igvm: igvm_path,
        disk: disk_path,
        disk_format: qemu_format.to_string(),
        smp: manifest.build.smp,
        memory: manifest.build.memory,
        port_forwards: vec![],
    };
    crate::qemu::launch(&qemu_args)?;

    Ok(())
}
