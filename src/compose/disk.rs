use std::path::Path;

use crate::tools;

/// Compose a final GPT disk image from base and project partitions.
///
/// The base partition (ext4, from `steep base`) becomes the root partition,
/// and the project partition (vfat cidata) becomes a secondary partition.
/// The output is a raw GPT disk image.
pub fn compose(
    base_partition: &Path,
    project_partition: &Path,
    output: &Path,
) -> anyhow::Result<()> {
    tracing::info!(
        base = %base_partition.display(),
        project = %project_partition.display(),
        output = %output.display(),
        "composing disk image"
    );

    if !base_partition.exists() {
        anyhow::bail!("base partition not found: {}", base_partition.display());
    }
    if !project_partition.exists() {
        anyhow::bail!("project partition not found: {}", project_partition.display());
    }

    tools::require("sfdisk")?;

    let project_meta = fs_err::metadata(project_partition)?;
    let project_size = project_meta.len();

    // The base_partition is a GPT disk image from mkosi with one ext4 partition.
    // Parse its partition table to find the ext4 content offset and size.
    let sector_size: u64 = 512;
    let sfdisk_output = tools::run_command(
        "sfdisk",
        &["--dump", &base_partition.display().to_string()],
    )?;

    let (base_start_sector, base_size_sectors) = parse_first_partition(&sfdisk_output)?;

    // Layout for new disk:
    //   GPT header: sectors 0-33
    //   Partition 1 (root/ext4): starts at sector 2048 (1MiB aligned)
    //   Partition 2 (cidata/vfat): starts after partition 1, aligned to 1MiB
    //   GPT backup: last 33 sectors
    let part1_start: u64 = 2048;
    let part1_sectors = base_size_sectors;
    let part1_end = part1_start + part1_sectors; // exclusive

    // Align partition 2 start to 1MiB boundary (2048 sectors)
    let part2_start = (part1_end + 2047) & !2047;
    let project_sectors = (project_size + sector_size - 1) / sector_size;
    let part2_end = part2_start + project_sectors;

    // Total disk size: partitions + GPT backup (34 sectors)
    let total_sectors = part2_end + 34;
    let total_size = total_sectors * sector_size;

    // Create output file
    let f = fs_err::File::create(output)?;
    f.set_len(total_size)?;
    drop(f);

    // Write partition table with sfdisk
    let sfdisk_script = format!(
        "label: gpt\n\
         start={}, size={}, type=4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709, name=\"Root Partition\"\n\
         start={}, size={}, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"Project Partition\"\n",
        part1_start, part1_sectors, part2_start, project_sectors,
    );

    let mut sfdisk_proc = std::process::Command::new("sfdisk")
        .arg(&output.display().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to run sfdisk: {e}"))?;

    use std::io::Write;
    sfdisk_proc
        .stdin
        .as_mut()
        .unwrap()
        .write_all(sfdisk_script.as_bytes())?;
    drop(sfdisk_proc.stdin.take());

    let sfdisk_result = sfdisk_proc.wait_with_output()?;
    if !sfdisk_result.status.success() {
        let stderr = String::from_utf8_lossy(&sfdisk_result.stderr);
        anyhow::bail!("sfdisk failed: {}", stderr);
    }

    // Copy base partition content (skip GPT header from source)
    tools::run_command(
        "dd",
        &[
            &format!("if={}", base_partition.display()),
            &format!("of={}", output.display()),
            &format!("bs={}", sector_size),
            &format!("skip={}", base_start_sector),
            &format!("seek={}", part1_start),
            &format!("count={}", part1_sectors),
            "conv=notrunc",
            "status=none",
        ],
    )?;

    // Copy project partition content
    tools::run_command(
        "dd",
        &[
            &format!("if={}", project_partition.display()),
            &format!("of={}", output.display()),
            "bs=512",
            &format!("seek={}", part2_start),
            "conv=notrunc",
            "status=none",
        ],
    )?;

    tracing::info!(
        sectors = total_sectors,
        "disk image composed with 2 partitions"
    );

    Ok(())
}

/// Parse sfdisk --dump output to find the first partition's start and size (in sectors).
fn parse_first_partition(sfdisk_output: &str) -> anyhow::Result<(u64, u64)> {
    for line in sfdisk_output.lines() {
        // Lines look like: "/path/to/image1 : start=          40, size=     6291456, ..."
        if !line.contains("start=") || !line.contains("size=") {
            continue;
        }
        let start = extract_field(line, "start=")?;
        let size = extract_field(line, "size=")?;
        return Ok((start, size));
    }
    anyhow::bail!("no partition found in sfdisk output")
}

fn extract_field(line: &str, field: &str) -> anyhow::Result<u64> {
    let rest = line
        .split(field)
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("field {field} not found"))?;
    let value_str = rest
        .split(',')
        .next()
        .unwrap()
        .trim();
    value_str
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("failed to parse {field} value: {value_str}"))
}
