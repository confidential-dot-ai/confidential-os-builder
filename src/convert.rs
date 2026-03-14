use std::path::Path;

use crate::ImageFormat;

/// Return the qemu-img format string for an ImageFormat.
pub fn qemu_img_format(format: &ImageFormat) -> &'static str {
    match format {
        ImageFormat::Qcow2 => "qcow2",
        ImageFormat::Vhd => "vpc",
        ImageFormat::Raw => "raw",
    }
}

/// Build the argument list for qemu-img convert.
pub fn convert_args(input: &Path, output: &Path, format: &ImageFormat) -> Vec<String> {
    vec![
        "convert".to_string(),
        "-f".to_string(),
        "raw".to_string(),
        "-O".to_string(),
        qemu_img_format(format).to_string(),
        input.display().to_string(),
        output.display().to_string(),
    ]
}

/// Convert a raw disk image to the specified format using qemu-img.
/// No-op if format is raw (copies the file instead).
pub fn convert(input: &Path, output: &Path, format: &ImageFormat) -> anyhow::Result<()> {
    if matches!(format, ImageFormat::Raw) {
        tracing::info!("output format is raw, skipping conversion");
        fs_err::copy(input, output)?;
        return Ok(());
    }
    crate::tools::require("qemu-img")?;
    let args = convert_args(input, output, format);
    tracing::info!(
        input = %input.display(),
        output = %output.display(),
        format = qemu_img_format(format),
        "converting disk image"
    );
    crate::tools::run_command_streaming("qemu-img", &args)?;
    Ok(())
}
