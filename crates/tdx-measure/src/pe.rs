//! PE/COFF parsing and Authenticode SHA-384 hash computation.
//!
//! The Authenticode hash excludes the PE checksum field and the
//! certificate table directory entry, then hashes headers followed
//! by sections sorted by file offset.

use std::mem;
use std::str;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha384};

fn read_le<T: FromLeBytes>(data: &[u8], offset: usize) -> Result<T> {
    let size = mem::size_of::<T>();
    let end = offset.checked_add(size).context("offset overflow in read_le")?;
    if end > data.len() {
        bail!(
            "Read out of bounds at offset 0x{:x} (need {} bytes, have {})",
            offset,
            size,
            data.len()
        );
    }
    Ok(T::from_le_bytes_slice(&data[offset..end]))
}

trait FromLeBytes: Sized {
    fn from_le_bytes_slice(data: &[u8]) -> Self;
}

impl FromLeBytes for u16 {
    fn from_le_bytes_slice(data: &[u8]) -> Self {
        // SAFETY: read_le guarantees data.len() >= 2
        u16::from_le_bytes(data[..2].try_into().unwrap())
    }
}

impl FromLeBytes for u32 {
    fn from_le_bytes_slice(data: &[u8]) -> Self {
        // SAFETY: read_le guarantees data.len() >= 4
        u32::from_le_bytes(data[..4].try_into().unwrap())
    }
}

/// Compute the PE Authenticode SHA-384 hash.
///
/// This is what UEFI uses when measuring PE images into RTMRs.
/// It excludes the checksum field and certificate table entry.
pub fn authenticode_sha384(data: &[u8]) -> Result<Vec<u8>> {
    let lfanew: u32 = read_le(data, 0x3C)?;
    let pe_sig_offset = usize::try_from(lfanew).context("PE offset exceeds addressable range")?;

    let pe_sig: u32 = read_le(data, pe_sig_offset)?;
    if pe_sig != 0x0000_4550 {
        bail!("Invalid PE signature: 0x{:08x}", pe_sig);
    }

    let coff_offset = pe_sig_offset + 4;
    let opt_hdr_size: u16 = read_le(data, coff_offset + 16)?;
    let opt_hdr_offset = coff_offset + 20;

    let magic: u16 = read_le(data, opt_hdr_offset)?;
    let is_pe32_plus = magic == 0x20b;

    // Checksum field: optional header + 64
    let checksum_offset = opt_hdr_offset + 64;
    let checksum_end = checksum_offset + 4;

    // Certificate table entry: data directory index 4, each entry is 8 bytes
    let data_dir_offset = opt_hdr_offset + if is_pe32_plus { 112 } else { 96 };
    let cert_dir_offset = data_dir_offset + 4 * 8; // IMAGE_DIRECTORY_ENTRY_SECURITY = 4
    let cert_dir_end = cert_dir_offset + 8;

    // Size of headers
    let size_of_headers: u32 = read_le(data, opt_hdr_offset + 60)?;
    let size_of_headers = usize::try_from(size_of_headers).context("PE SizeOfHeaders exceeds addressable range")?;

    // Validate all header offsets are within data before slicing
    if cert_dir_end > data.len() {
        bail!(
            "PE cert directory entry (offset 0x{:x}) extends beyond file ({} bytes)",
            cert_dir_end,
            data.len()
        );
    }
    if size_of_headers > data.len() {
        bail!(
            "PE SizeOfHeaders ({}) exceeds file size ({})",
            size_of_headers,
            data.len()
        );
    }
    if size_of_headers < cert_dir_end {
        bail!(
            "PE SizeOfHeaders ({}) is smaller than cert directory end (0x{:x})",
            size_of_headers,
            cert_dir_end
        );
    }

    let mut hasher = Sha384::new();

    // Hash headers, skipping checksum and cert table entry
    hasher.update(&data[..checksum_offset]);
    hasher.update(&data[checksum_end..cert_dir_offset]);
    hasher.update(&data[cert_dir_end..size_of_headers]);

    let mut sum_of_bytes_hashed = size_of_headers;

    // Parse section table
    let num_sections: u16 = read_le(data, coff_offset + 2)?;
    let opt_hdr_size_usize = usize::from(opt_hdr_size);
    let section_table_offset = opt_hdr_offset + opt_hdr_size_usize;

    let num_sections_usize = usize::from(num_sections);
    let mut sections = Vec::with_capacity(num_sections_usize);
    for i in 0..num_sections_usize {
        let sec_offset = section_table_offset + i * 40;
        let ptr_raw_data: u32 = read_le(data, sec_offset + 20)?;
        let size_raw_data: u32 = read_le(data, sec_offset + 16)?;
        if size_raw_data > 0 {
            sections.push((
                usize::try_from(ptr_raw_data).context("section PointerToRawData overflow")?,
                usize::try_from(size_raw_data).context("section SizeOfRawData overflow")?,
            ));
        }
    }

    // Sort sections by file offset
    sections.sort_by_key(|&(offset, _)| offset);

    // Hash each section's raw data
    for (offset, size) in &sections {
        let end = offset.checked_add(*size)
            .context("section offset+size overflow")?;
        if end > data.len() {
            bail!(
                "PE section at offset 0x{:x} extends beyond file (need 0x{:x}, have 0x{:x})",
                offset,
                end,
                data.len()
            );
        }
        hasher.update(&data[*offset..end]);
        sum_of_bytes_hashed += size;
    }

    // Hash trailing data (excluding certificate table if present).
    // Per Authenticode spec: "If FILE_SIZE > SUM_OF_BYTES_HASHED, hash the remaining bytes."
    // The certificate table (at cert_table_addr, cert_table_size bytes) is excluded.
    let cert_table_addr: u32 = read_le(data, cert_dir_offset)?;
    let cert_table_size: u32 = read_le(data, cert_dir_offset + 4)?;

    if data.len() > sum_of_bytes_hashed {
        if cert_table_addr > 0 && cert_table_size > 0 {
            let ct_addr = usize::try_from(cert_table_addr)
                .context("cert table address overflow")?;
            let ct_size = usize::try_from(cert_table_size)
                .context("cert table size overflow")?;
            let ct_end = ct_addr.checked_add(ct_size)
                .context("cert table addr + size overflow")?;
            // Hash trailing data before the certificate table
            if ct_addr > sum_of_bytes_hashed {
                hasher.update(&data[sum_of_bytes_hashed..ct_addr]);
            }
            // Hash trailing data after the certificate table
            if ct_end < data.len() {
                hasher.update(&data[ct_end..]);
            }
        } else {
            // No certificate table — hash all trailing data
            hasher.update(&data[sum_of_bytes_hashed..]);
        }
    }

    // DEVIATION: OVMF pads the Authenticode hash to 8-byte alignment of file size.
    // The Microsoft Authenticode spec does not require this, but OVMF's
    // PeHashImage() implementation does it. Without this padding, computed
    // hashes do not match OVMF's measurements.
    let remainder = data.len() % 8;
    if remainder != 0 {
        hasher.update(&[0u8; 7][..8 - remainder]);
    }

    Ok(hasher.finalize().to_vec())
}

/// Parse PE sections, returning (name, data) pairs.
/// Uses virtual_size for the data length (not raw_size, which includes padding).
pub fn parse_sections(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let lfanew: u32 = read_le(data, 0x3C).context("Reading PE offset")?;
    let pe_offset = usize::try_from(lfanew).context("PE offset exceeds addressable range")?;

    if data.len() < pe_offset + 4 || &data[pe_offset..pe_offset + 4] != b"PE\x00\x00" {
        bail!("Invalid PE signature");
    }

    let coff_offset = pe_offset + 4;
    let num_sections: u16 = read_le(data, coff_offset + 2)?;
    let opt_hdr_size: u16 = read_le(data, coff_offset + 16)?;
    let section_table = coff_offset + 20 + usize::from(opt_hdr_size);

    let mut sections = Vec::new();
    for i in 0..usize::from(num_sections) {
        let off = section_table + i * 40;
        if off + 40 > data.len() {
            break;
        }

        // Section name: 8 bytes, NUL-padded
        let name_bytes = &data[off..off + 8];
        let name = str::from_utf8(
            &name_bytes[..name_bytes.iter().position(|&b| b == 0).unwrap_or(8)],
        )
        .unwrap_or("")
        .to_string();

        let virtual_size: u32 = read_le(data, off + 8)?;
        let raw_offset: u32 = read_le(data, off + 20)?;

        let start = usize::try_from(raw_offset).context("section raw offset overflow")?;
        let vsize = usize::try_from(virtual_size).context("section virtual size overflow")?;
        let end = start.checked_add(vsize)
            .context("section offset + virtual_size overflow")?;
        if end > data.len() {
            bail!(
                "PE section '{}' (offset 0x{:x}, virtual_size 0x{:x}) extends beyond file (0x{:x})",
                name, start, vsize, data.len()
            );
        }
        sections.push((name, data[start..end].to_vec()));
    }

    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_le_u16() {
        let data = [0x34, 0x12, 0xFF];
        let val: u16 = read_le(&data, 0).unwrap();
        assert_eq!(val, 0x1234);
    }

    #[test]
    fn test_read_le_u32() {
        let data = [0x78, 0x56, 0x34, 0x12];
        let val: u32 = read_le(&data, 0).unwrap();
        assert_eq!(val, 0x12345678);
    }

    #[test]
    fn test_read_le_out_of_bounds() {
        let data = [0x00, 0x01];
        let result: Result<u32> = read_le(&data, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_le_offset_overflow() {
        let data = [0x00; 8];
        let result: Result<u32> = read_le(&data, usize::MAX);
        assert!(result.is_err());
    }

    #[test]
    fn test_authenticode_rejects_non_pe() {
        let data = vec![0u8; 256];
        let result = authenticode_sha384(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_authenticode_rejects_truncated() {
        let data = vec![0u8; 16]; // way too small
        let result = authenticode_sha384(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_sections_rejects_non_pe() {
        let data = vec![0u8; 256];
        let result = parse_sections(&data);
        assert!(result.is_err());
    }

    /// Build a minimal valid PE32+ binary for testing.
    fn build_minimal_pe() -> Vec<u8> {
        let mut pe = vec![0u8; 512];

        // DOS header: e_lfanew at offset 0x3C
        pe[0] = b'M';
        pe[1] = b'Z';
        let pe_offset: u32 = 0x80;
        pe[0x3C..0x40].copy_from_slice(&pe_offset.to_le_bytes());

        let po = pe_offset as usize;

        // PE signature
        pe[po..po + 4].copy_from_slice(b"PE\x00\x00");

        // COFF header
        let coff = po + 4;
        pe[coff + 2..coff + 4].copy_from_slice(&0u16.to_le_bytes()); // 0 sections
        pe[coff + 16..coff + 18].copy_from_slice(&112u16.to_le_bytes()); // opt header size (PE32+)

        // Optional header
        let opt = coff + 20;
        pe[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes()); // PE32+ magic

        // SizeOfHeaders
        pe[opt + 60..opt + 64].copy_from_slice(&512u32.to_le_bytes());

        // Data directories: need at least 5 (index 0-4), each 8 bytes
        // cert table at index 4: offset = opt + 112 + 4*8 = opt + 144
        // But we need cert_dir_end <= data.len(), so make sure we have enough space

        pe
    }

    #[test]
    fn test_authenticode_minimal_pe() {
        let pe = build_minimal_pe();
        let result = authenticode_sha384(&pe);
        // Should succeed on a valid (if empty) PE
        assert!(result.is_ok(), "authenticode_sha384 failed: {:?}", result.err());
        assert_eq!(result.unwrap().len(), 48);
    }

    #[test]
    fn test_authenticode_deterministic() {
        let pe = build_minimal_pe();
        let h1 = authenticode_sha384(&pe).unwrap();
        let h2 = authenticode_sha384(&pe).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_authenticode_trailing_data_hashed() {
        // Verify that trailing data after the last section is included in the hash
        let pe1 = build_minimal_pe();
        let mut pe2 = pe1.clone();
        pe2.extend_from_slice(&[0xFF; 16]); // append trailing data

        let h1 = authenticode_sha384(&pe1).unwrap();
        let h2 = authenticode_sha384(&pe2).unwrap();
        // Hashes should differ because trailing data is hashed
        assert_ne!(h1, h2, "trailing data should affect Authenticode hash");
    }

    /// Pin the exact Authenticode hash — catches any drift in the hash algorithm.
    #[test]
    fn test_authenticode_golden_value() {
        let pe = build_minimal_pe();
        let hash = hex::encode(authenticode_sha384(&pe).unwrap());
        assert_eq!(
            hash,
            "ec61d8f49c45551513fade57216dee6f9723101e285c5a9c8dae14ccebae7e69cc65a9f6ee3560737cc394f457e2098a",
        );
    }

    /// Pin the Authenticode hash of PE with trailing data.
    #[test]
    fn test_authenticode_trailing_golden_value() {
        let mut pe = build_minimal_pe();
        pe.extend_from_slice(&[0xFF; 16]);
        let hash = hex::encode(authenticode_sha384(&pe).unwrap());
        assert_eq!(
            hash,
            "9f0bcf7844630269a51bd33ad1fa416bf1d13daa9323dec9b752a9d5c124cb62574acbc8e7becac08522801cfba88224",
        );
    }
}
