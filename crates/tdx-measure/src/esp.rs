//! ESP (EFI System Partition) inspection.
//!
//! The boot path TDVF actually walks on our disks is:
//!   TDVF → `/EFI/BOOT/BOOTX64.EFI` (systemd-boot fallback) → UKI in
//!   `/EFI/Linux/<entry>.efi`
//!
//! Each PE load is measured as a separate
//! `EV_EFI_BOOT_SERVICES_APPLICATION` event extended into RTMR[1]. The
//! UKI's Authenticode hash is easy to compute (we have the .efi as
//! `output/<name>/uki.efi`), but systemd-boot's bytes live inside the
//! disk's ESP — there's no separate "bootloader" file step in the steep
//! pipeline. This module pulls the bootloader bytes out of the disk
//! image at measurement time.
//!
//! On steep-built disks the first partition is the ESP, formatted FAT32,
//! and the fallback bootloader is at `/EFI/BOOT/BOOTX64.EFI` per the UEFI
//! removable-media boot policy (which is what TDVF chases since the
//! variable store is fresh and no Boot#### entry points elsewhere).

use std::io::Cursor;

use anyhow::{bail, Context, Result};

/// Path to the UEFI removable-media fallback bootloader inside the ESP.
/// Same path mkosi uses when it lays down systemd-boot.
pub const FALLBACK_BOOTLOADER_PATH: &str = "EFI/BOOT/BOOTX64.EFI";

/// Read a file out of the ESP partition embedded in a raw disk image.
///
/// Walks the GPT to find the first ESP-typed partition, opens its FAT32
/// volume read-only, and returns the file bytes verbatim. The disk image
/// must be a complete GPT-partitioned raw disk (sector 0 = protective
/// MBR, sector 1 = GPT header).
pub fn read_esp_file(disk: &[u8], path: &str) -> Result<Vec<u8>> {
    // GPT header lives at LBA 1 (offset 512). We re-parse it here rather
    // than reuse `rtmr::compute_gpt_event_hash`'s parser because that
    // one returns the measurement hash, not the partition table fields
    // we need to find the ESP boundaries.
    if disk.len() < 1024 {
        bail!("disk image too small for a GPT header");
    }
    let gpt_header = &disk[512..512 + 92];
    if &gpt_header[0..8] != b"EFI PART" {
        bail!("disk image does not start with a GPT (no `EFI PART` signature at LBA 1)");
    }
    let partition_entry_lba = u64::from_le_bytes(
        gpt_header[72..80]
            .try_into()
            .context("reading partition_entry_lba from GPT header")?,
    );
    let num_entries = u32::from_le_bytes(
        gpt_header[80..84]
            .try_into()
            .context("reading number_of_partition_entries from GPT header")?,
    );
    let entry_size = u32::from_le_bytes(
        gpt_header[84..88]
            .try_into()
            .context("reading size_of_partition_entry from GPT header")?,
    );
    if entry_size < 128 {
        bail!(
            "GPT partition entry size {} below the 128-byte minimum",
            entry_size
        );
    }

    // EFI System Partition GUID per the UEFI spec, little-endian on disk.
    const ESP_GUID: [u8; 16] = [
        0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9,
        0x3b,
    ];

    let entries_off = (partition_entry_lba as usize) * 512;
    let entry_size = entry_size as usize;
    let mut esp_first_lba: Option<u64> = None;
    let mut esp_last_lba: Option<u64> = None;
    for i in 0..(num_entries as usize) {
        let off = entries_off + i * entry_size;
        if off + entry_size > disk.len() {
            break;
        }
        let entry = &disk[off..off + entry_size];
        if entry[0..16] != ESP_GUID {
            continue;
        }
        esp_first_lba = Some(u64::from_le_bytes(
            entry[32..40]
                .try_into()
                .context("reading partition first_lba")?,
        ));
        esp_last_lba = Some(u64::from_le_bytes(
            entry[40..48]
                .try_into()
                .context("reading partition last_lba")?,
        ));
        break;
    }
    let (first_lba, last_lba) = match (esp_first_lba, esp_last_lba) {
        (Some(a), Some(b)) => (a, b),
        _ => bail!("no ESP partition found in the disk image's GPT"),
    };

    let start = first_lba as usize * 512;
    let end = (last_lba + 1) as usize * 512;
    if end > disk.len() {
        bail!(
            "GPT claims ESP runs to byte {} but disk image is only {} bytes",
            end,
            disk.len()
        );
    }
    let esp_slice = &disk[start..end];

    // fatfs wants a Read + Write + Seek. Vec<u8> over the ESP slice gives
    // us all three; nothing here writes, but the trait bound requires it.
    let cursor = Cursor::new(esp_slice.to_vec());
    let fs = fatfs::FileSystem::new(cursor, fatfs::FsOptions::new())
        .context("opening FAT32 filesystem on ESP partition")?;
    let root = fs.root_dir();

    // Walk the path component-by-component. fatfs's open_file takes a
    // single relative segment per call, not a slash-delimited path.
    let mut cur_dir = root;
    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let (file_name, dir_components) = components
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("empty path"))?;
    for comp in dir_components {
        cur_dir = cur_dir
            .open_dir(comp)
            .with_context(|| format!("opening ESP directory {comp:?} while resolving {path:?}"))?;
    }
    let mut file = cur_dir
        .open_file(file_name)
        .with_context(|| format!("opening ESP file {path:?}"))?;

    use std::io::Read;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("reading ESP file {path:?}"))?;
    Ok(bytes)
}
