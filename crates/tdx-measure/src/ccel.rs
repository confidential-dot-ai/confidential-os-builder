//! CCEL (CC Event Log) parsing and RTMR replay.
//!
//! The CCEL is a TCG2 format event log stored in the ACPI table
//! at /sys/firmware/acpi/tables/data/CCEL. Each event contains
//! an MR index (1-4 mapping to RTMR[0-3]) and SHA-384 digest.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha384};
use subtle::ConstantTimeEq;

/// Maximum number of digest algorithms per event (TCG spec uses ~3).
const MAX_DIGEST_ALGORITHMS: u32 = 16;

/// Maximum number of events to parse (DoS protection for untrusted input).
const MAX_EVENTS: usize = 10_000;

/// A parsed CCEL event.
#[derive(Debug)]
pub struct CcelEvent {
    pub mr_index: u32,
    pub event_type: u32,
    pub sha384_digest: Vec<u8>,
    pub event_data: Vec<u8>,
}

/// Parse a CCEL binary into a list of events.
///
/// Returns an error if the event log is truncated (ends without a proper
/// terminator record). This prevents silent partial results.
pub fn parse_ccel(data: &[u8]) -> Result<Vec<CcelEvent>> {
    if data.len() < 32 {
        bail!("CCEL data too short: {} bytes", data.len());
    }

    // Validate Spec ID Event header: EventType must be EV_NO_ACTION (3)
    let spec_event_type = u32::from_le_bytes(
        data[4..8]
            .try_into()
            .context("reading Spec ID Event type")?,
    );
    if spec_event_type != 3 {
        bail!(
            "Invalid Spec ID Event: expected EV_NO_ACTION (3), got 0x{:08x}",
            spec_event_type
        );
    }

    // Skip Spec ID Event header
    let event_size = usize::try_from(u32::from_le_bytes(
        data[28..32]
            .try_into()
            .context("reading Spec ID Event size")?,
    ))
    .context("Spec ID Event size overflow")?;
    let offset = 32usize
        .checked_add(event_size)
        .context("Spec ID Event size overflow")?;
    if offset > data.len() {
        bail!(
            "Spec ID Event size ({}) exceeds CCEL data ({})",
            event_size,
            data.len()
        );
    }
    let mut offset = offset;

    let mut events = Vec::new();
    let mut terminated = false;

    while offset < data.len() {
        if offset + 8 > data.len() {
            break; // truncated
        }

        let mr_index = u32::from_le_bytes(
            data[offset..offset + 4]
                .try_into()
                .context("reading MR index")?,
        );

        // UEFI firmware pads unused event log space with 0xFF.
        // Treat 0xFFFFFFFF mr_index as end-of-log.
        if mr_index == 0xFFFF_FFFF {
            terminated = true;
            break;
        }

        let event_type = u32::from_le_bytes(
            data[offset + 4..offset + 8]
                .try_into()
                .context("reading event type")?,
        );

        let mut pos = offset + 8;
        if pos + 4 > data.len() {
            break; // truncated
        }

        let digest_count = u32::from_le_bytes(
            data[pos..pos + 4]
                .try_into()
                .context("reading digest count")?,
        );
        pos += 4;

        if digest_count > MAX_DIGEST_ALGORITHMS {
            bail!(
                "Unreasonable digest count {} at offset 0x{:x}",
                digest_count,
                offset
            );
        }

        let mut sha384_digest = Vec::new();
        let mut digest_parse_ok = true;

        for _ in 0..digest_count {
            if pos + 2 > data.len() {
                digest_parse_ok = false;
                break;
            }
            let algo_id =
                u16::from_le_bytes(data[pos..pos + 2].try_into().context("reading algo ID")?);
            pos += 2;

            let digest_size = match algo_id {
                0x000C => 48, // SHA-384
                0x000B => 32, // SHA-256
                0x0004 => 20, // SHA-1
                _ => {
                    bail!(
                        "Unknown digest algorithm 0x{:04x} at offset 0x{:x}; \
                         cannot determine digest size",
                        algo_id,
                        offset
                    );
                }
            };

            if pos + digest_size > data.len() {
                digest_parse_ok = false;
                break;
            }
            if algo_id == 0x000C {
                sha384_digest = data[pos..pos + digest_size].to_vec();
            }
            pos += digest_size;
        }

        if !digest_parse_ok {
            break; // truncated
        }

        if pos + 4 > data.len() {
            break; // truncated
        }
        let event_data_size = usize::try_from(u32::from_le_bytes(
            data[pos..pos + 4]
                .try_into()
                .context("reading event data size")?,
        ))
        .context("event data size overflow")?;
        pos += 4;

        if pos + event_data_size > data.len() {
            break; // truncated
        }

        // Terminator check
        if event_type == 0 && mr_index == 0 && event_data_size == 0 {
            terminated = true;
            break;
        }

        let event_data = data[pos..pos + event_data_size].to_vec();

        if !sha384_digest.is_empty() {
            if events.len() >= MAX_EVENTS {
                bail!(
                    "CCEL event log exceeds maximum event count ({})",
                    MAX_EVENTS
                );
            }
            events.push(CcelEvent {
                mr_index,
                event_type,
                sha384_digest,
                event_data,
            });
        }

        offset = pos + event_data_size;
    }

    // If we consumed the entire buffer without a terminator, that's also ok
    // (some firmware doesn't emit a terminator). But if we broke out mid-event,
    // the data was truncated — warn callers.
    if !terminated && offset < data.len() {
        bail!(
            "CCEL event log truncated at offset 0x{:x} ({} events parsed before truncation)",
            offset,
            events.len()
        );
    }

    Ok(events)
}

/// Replay CCEL events to compute RTMR values.
///
/// Returns [RTMR[0], RTMR[1], RTMR[2], RTMR[3]].
/// RTMR extend: `RTMR_new = SHA384(RTMR_old || event_digest)`
pub fn replay_rtmrs(events: &[CcelEvent]) -> [Vec<u8>; 4] {
    let mut rtmrs: [Vec<u8>; 4] = [vec![0u8; 48], vec![0u8; 48], vec![0u8; 48], vec![0u8; 48]];

    for event in events {
        let idx = match event.mr_index {
            1 => 0,
            2 => 1,
            3 => 2,
            4 => 3,
            _ => continue,
        };

        let mut hasher = Sha384::new();
        hasher.update(&rtmrs[idx]);
        hasher.update(&event.sha384_digest);
        rtmrs[idx] = hasher.finalize().to_vec();
    }

    rtmrs
}

/// Extract RTMR[0..3] from a 1024-byte TDREPORT.
pub fn extract_rtmrs_from_tdreport(report: &[u8]) -> Result<[Vec<u8>; 4]> {
    const TDINFO_OFFSET: usize = 512;
    const TDREPORT_SIZE: usize = 1024;

    if report.len() != TDREPORT_SIZE {
        bail!(
            "TDREPORT must be exactly {} bytes, got {}",
            TDREPORT_SIZE,
            report.len()
        );
    }

    let offsets = [
        TDINFO_OFFSET + 208,
        TDINFO_OFFSET + 256,
        TDINFO_OFFSET + 304,
        TDINFO_OFFSET + 352,
    ];

    Ok(offsets.map(|off| report[off..off + 48].to_vec()))
}

/// Constant-time comparison of two digests.
/// Relies on `subtle::ConstantTimeEq` which handles mismatched lengths
/// in constant time (returns false without leaking which bytes differ).
pub fn digests_equal(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Parse the UEFI_VARIABLE_DATA structure from event data.
/// Returns (variable_name, variable_data) if the structure is valid.
pub fn parse_uefi_variable_data(event_data: &[u8]) -> Option<(String, &[u8])> {
    if event_data.len() < 32 {
        return None;
    }
    let name_len = usize::try_from(u64::from_le_bytes(event_data[16..24].try_into().ok()?)).ok()?;
    let data_len = usize::try_from(u64::from_le_bytes(event_data[24..32].try_into().ok()?)).ok()?;
    let name_end = 32usize.checked_add(name_len.checked_mul(2)?)?;
    let data_end = name_end.checked_add(data_len)?;
    if data_end > event_data.len() || name_end > event_data.len() {
        return None;
    }
    let var_name: String = event_data[32..name_end]
        .chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                char::from_u32(u16::from_le_bytes([c[0], c[1]]) as u32)
            } else {
                None
            }
        })
        .collect();
    Some((var_name, &event_data[name_end..data_end]))
}

/// Return a human-readable name for a TCG2 event type.
pub fn event_type_name(etype: u32) -> &'static str {
    match etype {
        0x0000_0000 => "EV_PREBOOT_CERT",
        0x0000_0001 => "EV_POST_CODE",
        0x0000_0003 => "EV_NO_ACTION",
        0x0000_0004 => "EV_SEPARATOR",
        0x0000_0005 => "EV_ACTION",
        0x0000_0006 => "EV_EVENT_TAG",
        0x0000_000A => "EV_PLATFORM_CONFIG_FLAGS",
        0x0000_000D => "EV_IPL",
        0x8000_0001 => "EV_EFI_VARIABLE_DRIVER_CONFIG",
        0x8000_0002 => "EV_EFI_VARIABLE_BOOT",
        0x8000_0003 => "EV_EFI_BOOT_SERVICES_APPLICATION",
        0x8000_0004 => "EV_EFI_BOOT_SERVICES_DRIVER",
        0x8000_0005 => "EV_EFI_RUNTIME_SERVICES_DRIVER",
        0x8000_0006 => "EV_EFI_GPT_EVENT",
        0x8000_0007 => "EV_EFI_ACTION",
        0x8000_000E => "EV_EFI_SPDM_FIRMWARE_BLOB",
        0x8000_0010 => "EV_EFI_HCRTM_EVENT",
        _ => "EV_UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid CCEL buffer with a Spec ID Event header and optional events.
    fn build_ccel(events: &[(u32, u32, &[u8], &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();

        // Spec ID Event header (32 bytes)
        buf.extend_from_slice(&0u32.to_le_bytes()); // PCRIndex = 0
        buf.extend_from_slice(&3u32.to_le_bytes()); // EventType = EV_NO_ACTION
        buf.extend_from_slice(&[0u8; 20]); // remaining header fields
        buf.extend_from_slice(&0u32.to_le_bytes()); // event_size = 0

        // Append events: (mr_index, event_type, sha384_digest, event_data)
        for &(mr_index, event_type, digest, event_data) in events {
            buf.extend_from_slice(&mr_index.to_le_bytes());
            buf.extend_from_slice(&event_type.to_le_bytes());
            buf.extend_from_slice(&1u32.to_le_bytes()); // digest count = 1
            buf.extend_from_slice(&0x000Cu16.to_le_bytes()); // algo = SHA-384
            buf.extend_from_slice(digest); // 48 bytes
            buf.extend_from_slice(&(event_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(event_data);
        }

        buf
    }

    /// Build a terminator event (mr_index=0, event_type=0, event_data_size=0).
    fn append_terminator(buf: &mut Vec<u8>) {
        buf.extend_from_slice(&0u32.to_le_bytes()); // mr_index = 0
        buf.extend_from_slice(&0u32.to_le_bytes()); // event_type = 0
        buf.extend_from_slice(&1u32.to_le_bytes()); // digest count = 1
        buf.extend_from_slice(&0x000Cu16.to_le_bytes()); // algo = SHA-384
        buf.extend_from_slice(&[0u8; 48]); // digest
        buf.extend_from_slice(&0u32.to_le_bytes()); // event_data_size = 0
    }

    #[test]
    fn test_parse_ccel_too_short() {
        let data = vec![0u8; 16];
        assert!(parse_ccel(&data).is_err());
    }

    #[test]
    fn test_parse_ccel_empty_log() {
        // Just a Spec ID header, no events — consuming entire buffer without terminator is ok
        let buf = build_ccel(&[]);
        let events = parse_ccel(&buf).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_ccel_single_event() {
        let digest = [0xAA; 48];
        let event_data = b"test event";
        let buf = build_ccel(&[(1, 0x80000003, &digest, event_data)]);
        let events = parse_ccel(&buf).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].mr_index, 1);
        assert_eq!(events[0].event_type, 0x80000003);
        assert_eq!(events[0].sha384_digest, digest.to_vec());
        assert_eq!(events[0].event_data, event_data.to_vec());
    }

    #[test]
    fn test_parse_ccel_with_terminator() {
        let digest = [0xBB; 48];
        let mut buf = build_ccel(&[(2, 0x00000004, &digest, b"sep")]);
        append_terminator(&mut buf);
        // Add garbage after terminator — should be ignored
        buf.extend_from_slice(&[0xFF; 64]);
        let events = parse_ccel(&buf).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_parse_ccel_truncated_mid_event() {
        let digest = [0xCC; 48];
        let mut buf = build_ccel(&[(1, 0x00000001, &digest, b"ok")]);
        // Append start of another event but truncate it
        buf.extend_from_slice(&1u32.to_le_bytes()); // mr_index
        buf.extend_from_slice(&5u32.to_le_bytes()); // event_type
                                                    // Stop here — missing digest_count
        let result = parse_ccel(&buf);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn test_parse_ccel_invalid_spec_id_event() {
        let buf = vec![0u8; 32]; // EventType at offset 4..8 = 0, not EV_NO_ACTION
        let result = parse_ccel(&buf);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("EV_NO_ACTION"));
    }

    #[test]
    fn test_parse_ccel_unknown_algo() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes()); // PCRIndex
        buf.extend_from_slice(&3u32.to_le_bytes()); // EventType = EV_NO_ACTION
        buf.extend_from_slice(&[0u8; 20]); // remaining header
        buf.extend_from_slice(&0u32.to_le_bytes()); // Spec ID size = 0
                                                    // Event with unknown algorithm
        buf.extend_from_slice(&1u32.to_le_bytes()); // mr_index
        buf.extend_from_slice(&1u32.to_le_bytes()); // event_type
        buf.extend_from_slice(&1u32.to_le_bytes()); // digest count
        buf.extend_from_slice(&0xFFFFu16.to_le_bytes()); // unknown algo
        let result = parse_ccel(&buf);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown digest algorithm"));
    }

    #[test]
    fn test_parse_ccel_skips_non_sha384() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_le_bytes()); // PCRIndex
        buf.extend_from_slice(&3u32.to_le_bytes()); // EventType = EV_NO_ACTION
        buf.extend_from_slice(&[0u8; 20]); // remaining header
        buf.extend_from_slice(&0u32.to_le_bytes()); // Spec ID size = 0
                                                    // Event with SHA-256 only (no SHA-384)
        buf.extend_from_slice(&1u32.to_le_bytes()); // mr_index
        buf.extend_from_slice(&1u32.to_le_bytes()); // event_type
        buf.extend_from_slice(&1u32.to_le_bytes()); // digest count = 1
        buf.extend_from_slice(&0x000Bu16.to_le_bytes()); // algo = SHA-256
        buf.extend_from_slice(&[0xDD; 32]); // 32-byte digest
        buf.extend_from_slice(&4u32.to_le_bytes()); // event_data_size
        buf.extend_from_slice(b"test");
        let events = parse_ccel(&buf).unwrap();
        // No SHA-384 digest means the event is skipped
        assert!(events.is_empty());
    }

    #[test]
    fn test_replay_rtmrs_empty() {
        let rtmrs = replay_rtmrs(&[]);
        for rtmr in &rtmrs {
            assert_eq!(rtmr, &vec![0u8; 48]);
        }
    }

    #[test]
    fn test_replay_rtmrs_single_event() {
        let digest = vec![0xAA; 48];
        let events = vec![CcelEvent {
            mr_index: 2, // RTMR[1]
            event_type: 0x80000003,
            sha384_digest: digest.clone(),
            event_data: vec![],
        }];
        let rtmrs = replay_rtmrs(&events);
        // RTMR[1] should be extended, others unchanged
        assert_eq!(rtmrs[0], vec![0u8; 48]); // RTMR[0] untouched
        assert_ne!(rtmrs[1], vec![0u8; 48]); // RTMR[1] extended
        assert_eq!(rtmrs[2], vec![0u8; 48]); // RTMR[2] untouched
        assert_eq!(rtmrs[3], vec![0u8; 48]); // RTMR[3] untouched

        // Verify the extension: SHA384(48_zeros || digest)
        let mut hasher = Sha384::new();
        hasher.update(&[0u8; 48]);
        hasher.update(&digest);
        assert_eq!(rtmrs[1], hasher.finalize().to_vec());
    }

    #[test]
    fn test_replay_rtmrs_invalid_index_skipped() {
        let events = vec![CcelEvent {
            mr_index: 0, // not 1-4, should be skipped
            event_type: 1,
            sha384_digest: vec![0xFF; 48],
            event_data: vec![],
        }];
        let rtmrs = replay_rtmrs(&events);
        // All zeros — event was skipped
        for rtmr in &rtmrs {
            assert_eq!(rtmr, &vec![0u8; 48]);
        }
    }

    #[test]
    fn test_replay_rtmrs_multiple_registers() {
        let events = vec![
            CcelEvent {
                mr_index: 1,
                event_type: 1,
                sha384_digest: vec![0x11; 48],
                event_data: vec![],
            },
            CcelEvent {
                mr_index: 3,
                event_type: 1,
                sha384_digest: vec![0x33; 48],
                event_data: vec![],
            },
            CcelEvent {
                mr_index: 4,
                event_type: 1,
                sha384_digest: vec![0x44; 48],
                event_data: vec![],
            },
        ];
        let rtmrs = replay_rtmrs(&events);
        assert_ne!(rtmrs[0], vec![0u8; 48]); // RTMR[0] extended
        assert_eq!(rtmrs[1], vec![0u8; 48]); // RTMR[1] untouched
        assert_ne!(rtmrs[2], vec![0u8; 48]); // RTMR[2] extended
        assert_ne!(rtmrs[3], vec![0u8; 48]); // RTMR[3] extended
    }

    #[test]
    fn test_extract_rtmrs_from_tdreport_too_short() {
        let report = vec![0u8; 100];
        assert!(extract_rtmrs_from_tdreport(&report).is_err());
    }

    #[test]
    fn test_extract_rtmrs_from_tdreport_valid() {
        let mut report = vec![0u8; 1024];
        // Write known values at the RTMR offsets within TDINFO
        let offsets = [
            512 + 208, // RTMR[0]
            512 + 256, // RTMR[1]
            512 + 304, // RTMR[2]
            512 + 352, // RTMR[3]
        ];
        for (i, &off) in offsets.iter().enumerate() {
            for j in 0..48 {
                report[off + j] = (i + 1) as u8;
            }
        }
        let rtmrs = extract_rtmrs_from_tdreport(&report).unwrap();
        assert_eq!(rtmrs[0], vec![1u8; 48]);
        assert_eq!(rtmrs[1], vec![2u8; 48]);
        assert_eq!(rtmrs[2], vec![3u8; 48]);
        assert_eq!(rtmrs[3], vec![4u8; 48]);
    }

    #[test]
    fn test_digests_equal_same() {
        let a = vec![0xAA; 48];
        assert!(digests_equal(&a, &a));
    }

    #[test]
    fn test_digests_equal_different() {
        let a = vec![0xAA; 48];
        let b = vec![0xBB; 48];
        assert!(!digests_equal(&a, &b));
    }

    #[test]
    fn test_digests_equal_different_length() {
        let a = vec![0xAA; 48];
        let b = vec![0xAA; 32];
        assert!(!digests_equal(&a, &b));
    }

    #[test]
    fn test_parse_uefi_variable_data_too_short() {
        let data = vec![0u8; 16];
        assert!(parse_uefi_variable_data(&data).is_none());
    }

    #[test]
    fn test_parse_uefi_variable_data_valid() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0u8; 16]); // GUID (16 bytes)
        let name = "Test";
        let var_data = b"\x01\x02\x03";
        buf.extend_from_slice(&(name.len() as u64).to_le_bytes()); // name_len
        buf.extend_from_slice(&(var_data.len() as u64).to_le_bytes()); // data_len
                                                                       // Name as UTF-16LE
        for ch in name.encode_utf16() {
            buf.extend_from_slice(&ch.to_le_bytes());
        }
        buf.extend_from_slice(var_data);

        let (parsed_name, parsed_data) = parse_uefi_variable_data(&buf).unwrap();
        assert_eq!(parsed_name, "Test");
        assert_eq!(parsed_data, var_data);
    }

    #[test]
    fn test_parse_uefi_variable_data_overflow() {
        let mut buf = vec![0u8; 16]; // GUID
        buf.extend_from_slice(&u64::MAX.to_le_bytes()); // name_len = huge
        buf.extend_from_slice(&1u64.to_le_bytes()); // data_len
        assert!(parse_uefi_variable_data(&buf).is_none());
    }

    #[test]
    fn test_event_type_name_known() {
        assert_eq!(event_type_name(0x00000004), "EV_SEPARATOR");
        assert_eq!(
            event_type_name(0x80000003),
            "EV_EFI_BOOT_SERVICES_APPLICATION"
        );
        assert_eq!(event_type_name(0x80000006), "EV_EFI_GPT_EVENT");
    }

    #[test]
    fn test_event_type_name_unknown() {
        assert_eq!(event_type_name(0xDEADBEEF), "EV_UNKNOWN");
    }
}
