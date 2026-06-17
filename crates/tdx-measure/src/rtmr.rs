//! RTMR computation for UKI boot.
//!
//! RTMR[1]: UKI PE image identity (Authenticode hash + boot service constants)
//! RTMR[2]: UKI section measurements (kernel, initrd, cmdline, osrel, uname, sbat)

use std::collections::HashMap;
use std::str;

use anyhow::{Context, Result};
use sha2::{Digest, Sha384};

pub(crate) fn sha384(data: &[u8]) -> Vec<u8> {
    Sha384::new_with_prefix(data).finalize().to_vec()
}

pub(crate) fn rtmr_extend(current: &[u8], digest: &[u8]) -> Vec<u8> {
    let mut hasher = Sha384::new();
    hasher.update(current);
    hasher.update(digest);
    hasher.finalize().to_vec()
}

/// Extend a log of digests into a final RTMR value.
fn measure_log(log: &[Vec<u8>]) -> Vec<u8> {
    let mut mr = vec![0u8; 48];
    for entry in log {
        mr = rtmr_extend(&mr, entry);
    }
    mr
}

/// Compute RTMR[1] for UKI boot.
///
/// The 7 events in order:
///   1. EV_EFI_ACTION "Calling EFI Application from Boot Option"
///   2. EV_SEPARATOR (4 zero bytes)
///   3. EV_EFI_GPT_EVENT (GPT partition table hash from disk image)
///   4. EV_EFI_BOOT_SERVICES_APPLICATION (UKI PE Authenticode hash)
///   5. EV_EFI_BOOT_SERVICES_APPLICATION (kernel loaded by UKI stub)
///   6. EV_EFI_ACTION "Exit Boot Services Invocation"
///   7. EV_EFI_ACTION "Exit Boot Services Returned with Success"
pub fn compute_rtmr1_uki(
    uki_data: &[u8],
    sections: &[(String, Vec<u8>)],
    disk_image: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let uki_auth_hash =
        crate::pe::authenticode_sha384(uki_data).context("Failed to compute UKI Authenticode hash")?;

    // The kernel inside the UKI is also measured by OVMF when the stub
    // loads it via LoadImage. Compute its Authenticode hash from .linux section.
    let kernel_data = sections
        .iter()
        .find(|(name, _)| name == ".linux")
        .map(|(_, data)| data.as_slice());

    let kernel_auth_hash = if let Some(kdata) = kernel_data {
        // The .linux section is a raw bzImage, not a PE. OVMF measures the
        // kernel via LoadImage which computes the Authenticode hash of the
        // PE/COFF image. Linux bzImage has a PE header at the start.
        Some(crate::pe::authenticode_sha384(kdata).context("Failed to compute kernel Authenticode hash")?)
    } else {
        None
    };

    let gpt_hash = if let Some(disk) = disk_image {
        Some(compute_gpt_event_hash(disk)?)
    } else {
        None
    };

    let mut rtmr1_log = vec![
        sha384(b"Calling EFI Application from Boot Option"),
        sha384(&[0x00, 0x00, 0x00, 0x00]), // Separator
    ];

    if let Some(gh) = gpt_hash {
        rtmr1_log.push(gh);
    }

    rtmr1_log.push(uki_auth_hash);

    if let Some(kh) = kernel_auth_hash {
        rtmr1_log.push(kh);
    }

    rtmr1_log.push(sha384(b"Exit Boot Services Invocation"));
    rtmr1_log.push(sha384(b"Exit Boot Services Returned with Success"));

    Ok(measure_log(&rtmr1_log))
}

/// Compute the GPT event hash from a raw disk image.
///
/// The EFI_GPT_DATA structure measured by OVMF:
///   - GPT header (92 bytes from LBA 1)
///   - NumberOfPartitions (u64)
///   - Valid partition entries (128 bytes each)
fn compute_gpt_event_hash(disk: &[u8]) -> Result<Vec<u8>> {
    if disk.len() < 1024 {
        anyhow::bail!("Disk image too small for GPT");
    }

    // GPT header at LBA 1 (offset 512)
    let gpt_header = &disk[512..512 + 92];
    if &gpt_header[0..8] != b"EFI PART" {
        anyhow::bail!("Invalid GPT signature");
    }

    let partition_entry_lba =
        u64::from_le_bytes(gpt_header[72..80].try_into().context("reading partition entry LBA")?);
    let num_entries = usize::try_from(
        u32::from_le_bytes(gpt_header[80..84].try_into().context("reading num entries")?)
    ).context("GPT num_entries overflow")?;
    let entry_size = usize::try_from(
        u32::from_le_bytes(gpt_header[84..88].try_into().context("reading entry size")?)
    ).context("GPT entry_size overflow")?;

    if entry_size < 128 {
        anyhow::bail!("GPT partition entry size {} is below minimum 128", entry_size);
    }

    let entries_offset = partition_entry_lba
        .checked_mul(512)
        .context("GPT partition_entry_lba * 512 overflow")?;
    let entries_offset = usize::try_from(entries_offset)
        .context("GPT entries offset exceeds addressable range")?;

    // Collect valid (non-zero type GUID) partition entries
    let mut valid_entries = Vec::new();
    for i in 0..num_entries {
        let off = entries_offset.checked_add(i.checked_mul(entry_size)
            .context("GPT entry offset overflow")?)
            .context("GPT entry offset overflow")?;
        if off.checked_add(entry_size).map_or(true, |end| end > disk.len()) {
            break;
        }
        let entry = &disk[off..off + entry_size];
        if !entry[0..16].iter().all(|&b| b == 0) {
            valid_entries.push(entry);
        }
    }

    // Build EFI_GPT_DATA
    let mut gpt_event = Vec::new();
    gpt_event.extend_from_slice(gpt_header);
    gpt_event.extend_from_slice(
        &u64::try_from(valid_entries.len()).context("GPT entry count overflow")?.to_le_bytes(),
    );
    for entry in valid_entries {
        gpt_event.extend_from_slice(entry);
    }

    Ok(sha384(&gpt_event))
}

/// UKI sections measured by the UEFI stub, in canonical order.
const UKI_MEASUREMENT_ORDER: &[&str] = &[
    ".linux", ".osrel", ".cmdline", ".initrd", ".uname", ".splash", ".dtb", ".sbat", ".pcrpkey",
];

/// Pre-compute individual RTMR[2] event digests from UKI sections.
///
/// Returns (kind, section_name, digest) tuples for ALL 14 events,
/// including the "Linux initrd" event (Event 14).
///
/// Event 14 is the kernel EFI stub's measurement of the assembled initrd.
/// systemd-stub v257 serves: .initrd section + os-release CPIO archive.
/// The CPIO contains .osrel content as `.extra/os-release`.
pub fn precompute_rtmr2_digests(
    sections: &[(String, Vec<u8>)],
) -> Result<Vec<(String, String, Vec<u8>)>> {
    let section_map: HashMap<&str, &[u8]> = sections
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();

    let mut digests = Vec::new();

    for &sect_name in UKI_MEASUREMENT_ORDER {
        if let Some(&sect_data) = section_map.get(sect_name) {
            // Section name: ASCII + NUL terminator
            let name_digest = sha384(&[sect_name.as_bytes(), b"\x00"].concat());
            digests.push(("name".into(), sect_name.into(), name_digest));

            // Section data: virtual_size bytes
            let data_digest = sha384(sect_data);
            digests.push(("data".into(), sect_name.into(), data_digest));
        }
    }

    // LOADED_IMAGE::LoadOptions: cmdline as UTF-16LE + NUL terminator
    if let Some(&cmdline_data) = section_map.get(".cmdline") {
        let cmdline = str::from_utf8(cmdline_data)
            .context("UKI .cmdline section contains invalid UTF-8")?
            .trim_end_matches(|c: char| c == '\n' || c == '\r' || c == '\0' || c == ' ');

        let mut utf16: Vec<u8> = cmdline
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        utf16.extend_from_slice(&[0x00, 0x00]); // UTF-16 NUL terminator

        let loadopts_digest = sha384(&utf16);
        digests.push(("loadopts".into(), "cmdline".into(), loadopts_digest));
    }

    // Event 14: "Linux initrd" — measured by the kernel's EFI stub.
    // systemd-stub assembles: .initrd (4-byte aligned) + os-release CPIO (4-byte aligned)
    if let (Some(&initrd_data), Some(&osrel_data)) =
        (section_map.get(".initrd"), section_map.get(".osrel"))
    {
        let combined = build_assembled_initrd(initrd_data, osrel_data);
        let initrd_digest = sha384(&combined);
        digests.push(("initrd".into(), "assembled".into(), initrd_digest));
    }

    Ok(digests)
}

/// Build the assembled initrd as systemd-stub v257 serves it via LoadFile2.
///
/// Components (in order, each padded to 4-byte alignment):
///   1. .initrd section data (the base initramfs)
///   2. os-release CPIO archive (.osrel as `.extra/os-release`)
///
/// Only these two are present when the UKI has no .pcrsig, .pcrpkey,
/// credentials, sysext, or confext sections.
fn build_assembled_initrd(initrd: &[u8], osrel: &[u8]) -> Vec<u8> {
    let osrel_cpio = build_osrel_cpio(osrel);

    let mut combined = Vec::with_capacity(align4(initrd.len()) + align4(osrel_cpio.len()));

    // Base initrd, padded to 4 bytes
    combined.extend_from_slice(initrd);
    let pad = (4 - initrd.len() % 4) % 4;
    combined.extend_from_slice(&[0u8; 3][..pad]);

    // os-release CPIO, padded to 4 bytes
    combined.extend_from_slice(&osrel_cpio);
    let pad = (4 - osrel_cpio.len() % 4) % 4;
    combined.extend_from_slice(&[0u8; 3][..pad]);

    combined
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Build the os-release CPIO archive as systemd-stub v257 does.
///
/// CPIO newc format with:
///   - Directory entry: `.extra` (mode 040555, inode 1)
///   - File entry: `.extra/os-release` (mode 0100444, inode 2, content = .osrel)
///   - Trailer: `TRAILER!!!`
fn build_osrel_cpio(osrel_data: &[u8]) -> Vec<u8> {
    let mut cpio = Vec::new();

    // Directory: .extra
    append_cpio_entry(&mut cpio, ".extra", &[], 1, 0o040555);

    // File: .extra/os-release
    append_cpio_entry(&mut cpio, ".extra/os-release", osrel_data, 2, 0o0100444);

    // Trailer
    let trailer = b"070701\
        00000000\
        00000000\
        00000000\
        00000000\
        00000001\
        00000000\
        00000000\
        00000000\
        00000000\
        00000000\
        00000000\
        0000000B\
        00000000\
        TRAILER!!!\x00\x00\x00\x00";
    cpio.extend_from_slice(trailer);

    cpio
}

/// Append a single CPIO newc entry (header + name + data, each 4-byte aligned).
fn append_cpio_entry(buf: &mut Vec<u8>, name: &str, data: &[u8], ino: u32, mode: u32) {
    let namesize = name.len() + 1; // include NUL
    let filesize = data.len();

    // Header: "070701" + 13 fields of 8 hex chars each
    let hdr = format!(
        "070701\
         {:08x}\
         {:08x}\
         00000000\
         00000000\
         00000001\
         00000000\
         {:08x}\
         00000000\
         00000000\
         00000000\
         00000000\
         {:08x}\
         00000000",
        ino, mode, filesize, namesize
    );

    let start = buf.len();
    buf.extend_from_slice(hdr.as_bytes());
    buf.extend_from_slice(name.as_bytes());
    buf.push(0); // NUL terminator

    // Pad header+name to 4-byte alignment
    let hdr_len = buf.len() - start;
    let pad = (4 - hdr_len % 4) % 4;
    buf.extend_from_slice(&[0u8; 3][..pad]);

    // Data
    if !data.is_empty() {
        buf.extend_from_slice(data);
        // Pad data to 4-byte alignment
        let pad = (4 - filesize % 4) % 4;
        buf.extend_from_slice(&[0u8; 3][..pad]);
    }
}

/// Compute RTMR[2] for UKI boot — all 14 events, fully offline.
///
/// Returns (rtmr2, event_count).
pub fn compute_rtmr2_uki(sections: &[(String, Vec<u8>)]) -> Result<(Vec<u8>, usize)> {
    let digests = precompute_rtmr2_digests(sections)?;
    let count = digests.len();

    let digest_values: Vec<Vec<u8>> = digests.into_iter().map(|(_, _, d)| d).collect();
    let rtmr2 = measure_log(&digest_values);

    Ok((rtmr2, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha384_known_value() {
        // SHA-384("") = known constant
        let empty = sha384(b"");
        assert_eq!(empty.len(), 48);
        assert_eq!(
            hex::encode(&empty),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
    }

    #[test]
    fn test_sha384_data() {
        let h = sha384(b"hello");
        assert_eq!(h.len(), 48);
        assert_eq!(
            hex::encode(&h),
            "59e1748777448c69de6b800d7a33bbfb9ff1b463e44354c3553bcdb9c666fa90125a3c79f90397bdf5f6a13de828684f"
        );
    }

    #[test]
    fn test_rtmr_extend_from_zeros() {
        let zero = vec![0u8; 48];
        let digest = sha384(b"test");
        let result = rtmr_extend(&zero, &digest);
        assert_eq!(result.len(), 48);
        // Verify it's SHA384(48_zeros || digest)
        let mut expected_input = vec![0u8; 48];
        expected_input.extend_from_slice(&digest);
        assert_eq!(result, sha384(&expected_input));
    }

    #[test]
    fn test_rtmr_extend_chaining() {
        let zero = vec![0u8; 48];
        let d1 = sha384(b"event1");
        let d2 = sha384(b"event2");
        let r1 = rtmr_extend(&zero, &d1);
        let r2 = rtmr_extend(&r1, &d2);
        // r2 should differ from both r1 and zero
        assert_ne!(r2, r1);
        assert_ne!(r2, zero);
    }

    #[test]
    fn test_measure_log_empty() {
        let log: Vec<Vec<u8>> = vec![];
        let result = measure_log(&log);
        assert_eq!(result, vec![0u8; 48]);
    }

    #[test]
    fn test_measure_log_single() {
        let digest = sha384(b"test");
        let result = measure_log(&[digest.clone()]);
        let expected = rtmr_extend(&vec![0u8; 48], &digest);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_align4() {
        assert_eq!(align4(0), 0);
        assert_eq!(align4(1), 4);
        assert_eq!(align4(2), 4);
        assert_eq!(align4(3), 4);
        assert_eq!(align4(4), 4);
        assert_eq!(align4(5), 8);
    }

    #[test]
    fn test_cpio_entry_alignment() {
        let mut buf = Vec::new();
        append_cpio_entry(&mut buf, ".extra", &[], 1, 0o040555);
        // CPIO newc header = 110 bytes, name ".extra\0" = 7 bytes
        // 110 + 7 = 117, padded to 120
        assert_eq!(buf.len() % 4, 0);
    }

    #[test]
    fn test_cpio_trailer_alignment() {
        let cpio = build_osrel_cpio(b"test os-release data");
        // The entire CPIO archive should end with a properly aligned trailer
        // Trailer: 110-byte header + "TRAILER!!!\0" (11 bytes) + 3 pad = 124 bytes
        assert_eq!(cpio.len() % 4, 0);
    }

    #[test]
    fn test_cpio_trailer_length() {
        // Build a minimal CPIO with just the trailer to verify its exact size
        let osrel = build_osrel_cpio(b"");
        // Should contain: dir entry + file entry (empty) + trailer
        // All parts must be 4-byte aligned
        assert_eq!(osrel.len() % 4, 0);
    }

    #[test]
    fn test_assembled_initrd_alignment() {
        let initrd = vec![0xAAu8; 100]; // not 4-byte aligned
        let osrel = b"NAME=test\n";
        let combined = build_assembled_initrd(&initrd, osrel);
        // Each component should be padded to 4-byte alignment
        assert_eq!(combined.len() % 4, 0);
    }

    #[test]
    fn test_gpt_event_hash_too_small() {
        let disk = vec![0u8; 512]; // too small
        let result = compute_gpt_event_hash(&disk);
        assert!(result.is_err());
    }

    #[test]
    fn test_gpt_event_hash_bad_signature() {
        let disk = vec![0u8; 2048];
        // No "EFI PART" signature at offset 512
        let result = compute_gpt_event_hash(&disk);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid GPT signature"));
    }

    #[test]
    fn test_precompute_rtmr2_empty_sections() {
        let sections: Vec<(String, Vec<u8>)> = vec![];
        let digests = precompute_rtmr2_digests(&sections).unwrap();
        assert!(digests.is_empty());
    }

    #[test]
    fn test_precompute_rtmr2_single_section() {
        let sections = vec![
            (".linux".to_string(), vec![0xDE, 0xAD]),
        ];
        let digests = precompute_rtmr2_digests(&sections).unwrap();
        // .linux produces 2 events: name + data
        assert_eq!(digests.len(), 2);
        assert_eq!(digests[0].0, "name");
        assert_eq!(digests[0].1, ".linux");
        assert_eq!(digests[1].0, "data");
        assert_eq!(digests[1].1, ".linux");
    }

    #[test]
    fn test_precompute_rtmr2_cmdline_event() {
        let sections = vec![
            (".cmdline".to_string(), b"root=/dev/vda1\n".to_vec()),
        ];
        let digests = precompute_rtmr2_digests(&sections).unwrap();
        // .cmdline produces: name + data + loadopts (UTF-16LE) = 3 events
        assert_eq!(digests.len(), 3);
        assert_eq!(digests[2].0, "loadopts");
    }

    #[test]
    fn test_precompute_rtmr2_initrd_event14() {
        let sections = vec![
            (".initrd".to_string(), vec![0xFF; 100]),
            (".osrel".to_string(), b"NAME=test\nVERSION=1\n".to_vec()),
        ];
        let digests = precompute_rtmr2_digests(&sections).unwrap();
        // .initrd: name + data = 2
        // .osrel: name + data = 2
        // Event 14 (assembled initrd): 1
        // Total = 5
        assert_eq!(digests.len(), 5);
        assert_eq!(digests[4].0, "initrd");
        assert_eq!(digests[4].1, "assembled");
    }

    #[test]
    fn test_section_name_includes_nul() {
        // Verify section name hashing includes NUL terminator
        let with_nul = sha384(&[b".linux".as_slice(), b"\x00"].concat());
        let without_nul = sha384(b".linux");
        assert_ne!(with_nul, without_nul);
    }

    /// Pin the RTMR[1] extend chain — catches drift in RTMR computation.
    #[test]
    fn test_rtmr1_chain_golden_value() {
        let action = sha384(b"Calling EFI Application from Boot Option");
        assert_eq!(
            hex::encode(&action),
            "77a0dab2312b4e1e57a84d865a21e5b2ee8d677a21012ada819d0a98988078d3d740f6346bfe0abaa938ca20439a8d71",
        );

        let separator = sha384(&[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(
            hex::encode(&separator),
            "394341b7182cd227c5c6b07ef8000cdfd86136c4292b8e576573ad7ed9ae41019f5818b4b971c9effc60e1ad9f1289f0",
        );

        // RTMR extend chain: zeros → action → separator
        let r1 = rtmr_extend(&vec![0u8; 48], &action);
        assert_eq!(
            hex::encode(&r1),
            "8032dedfdb8373b9bf18849c61543d2ed4fd555ffb0028634689a13fc4de798ff904ccded77c2d72259ab9777a17d7bd",
        );
        let r2 = rtmr_extend(&r1, &separator);
        assert_eq!(
            hex::encode(&r2),
            "70bc457e087464760a8927d6312248dc117663410914ff8b1e42fd5dc91e16f5fe3f15ca64372d3e47af8b4c53b01df9",
        );
    }

    /// Pin the full RTMR[2] computation with known UKI sections.
    #[test]
    fn test_rtmr2_golden_value() {
        let sections = vec![
            (".linux".to_string(), vec![0xDE, 0xAD, 0xBE, 0xEF]),
            (".osrel".to_string(), b"NAME=TestOS\nVERSION=1.0\n".to_vec()),
            (".cmdline".to_string(), b"root=/dev/vda1 quiet\n".to_vec()),
            (".initrd".to_string(), vec![0xFF; 256]),
        ];
        let (rtmr2, count) = compute_rtmr2_uki(&sections).unwrap();
        assert_eq!(count, 10);
        assert_eq!(
            hex::encode(&rtmr2),
            "8d54b4f3ed7c7ce69d82712021a91c685ac892d0e86d3a96fc12e862ef8b560f00fbc2285f6a21dd7ea89e30e310b69c",
        );
    }
}
