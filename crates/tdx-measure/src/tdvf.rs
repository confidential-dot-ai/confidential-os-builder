//! TDVF (TDX Virtual Firmware) parsing and MRTD computation.
//!
//! Parses the TDVF metadata from an OVMF firmware binary and computes
//! MRTD by simulating the TDX Module's page measurement algorithm:
//!   For each section: MEM.PAGE.ADD + MR.EXTEND per page
//!   MRTD = SHA384(all page operations concatenated)
//!
//! Also computes RTMR[0] with all 15 events:
//!   1-2.  TD-HOB + CFV
//!   3-7.  Secure Boot variables (SecureBoot, PK, KEK, db, dbx)
//!   8.    Separator
//!   9-11. ACPI tables (loader, rsdp, tables)
//!   12-15. Boot variables (BootOrder, Boot0001, Boot0000, Boot0002)

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha384};

use crate::rtmr::{rtmr_extend, sha384};

/// Pre-computed ACPI measurement hashes for RTMR[0].
///
/// Three SHA384 hashes of the ACPI fw_cfg blobs in measurement order:
///   1. loader:  etc/table-loader   (table_loader.bin)
///   2. rsdp:    etc/acpi/rsdp      (rsdp.bin)
///   3. tables:  etc/acpi/tables    (acpi_tables.bin)
///
/// These are deterministic per firmware binary + QEMU machine config.
/// Extract once from a reference boot's CCEL, reuse for all subsequent
/// measurements with the same firmware+config.
pub struct AcpiHashes {
    pub loader_hash: Vec<u8>,
    pub rsdp_hash: Vec<u8>,
    pub tables_hash: Vec<u8>,
}

impl AcpiHashes {
    /// Compute from raw ACPI blobs.
    pub fn from_files(tables_path: &Path, rsdp_path: &Path, loader_path: &Path) -> Result<Self> {
        let tables = fs::read(tables_path)
            .with_context(|| format!("failed to read ACPI tables: {}", tables_path.display()))?;
        let rsdp = fs::read(rsdp_path)
            .with_context(|| format!("failed to read ACPI RSDP: {}", rsdp_path.display()))?;
        let loader = fs::read(loader_path)
            .with_context(|| format!("failed to read ACPI loader: {}", loader_path.display()))?;
        Ok(Self {
            loader_hash: sha384(&loader),
            rsdp_hash: sha384(&rsdp),
            tables_hash: sha384(&tables),
        })
    }

    /// Extract ACPI hashes from a CCEL event log.
    ///
    /// The 3 ACPI events are type 0x0000000a (EV_PLATFORM_CONFIG_FLAGS)
    /// targeting MR index 1 (RTMR[0]), appearing after the separator.
    pub fn extract_from_ccel(ccel_data: &[u8]) -> Result<Self> {
        let events = crate::ccel::parse_ccel(ccel_data)?;

        // Find events targeting RTMR[0] with type 0x0a after separator
        let mut past_separator = false;
        let mut acpi_digests = Vec::new();

        for ev in &events {
            if ev.mr_index != 1 {
                continue;
            }
            if ev.event_type == 0x00000004 {
                past_separator = true;
                continue;
            }
            if past_separator && ev.event_type == 0x0000000a {
                acpi_digests.push(ev.sha384_digest.clone());
            }
        }

        if acpi_digests.len() < 3 {
            bail!(
                "Expected 3 ACPI events in CCEL, found {}",
                acpi_digests.len()
            );
        }
        for (i, d) in acpi_digests[..3].iter().enumerate() {
            if d.len() != 48 {
                bail!(
                    "ACPI digest {} from CCEL has wrong length: {} (expected 48)",
                    i,
                    d.len()
                );
            }
        }

        Ok(Self {
            loader_hash: acpi_digests[0].clone(),
            rsdp_hash: acpi_digests[1].clone(),
            tables_hash: acpi_digests[2].clone(),
        })
    }

    /// Save hashes to a file (hex format, one per line).
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = format!(
            "{}\n{}\n{}\n",
            hex::encode(&self.loader_hash),
            hex::encode(&self.rsdp_hash),
            hex::encode(&self.tables_hash),
        );
        fs::write(path, content)
            .with_context(|| format!("failed to write ACPI hashes: {}", path.display()))
    }

    /// Load hashes from a file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read ACPI hashes: {}", path.display()))?;
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() < 3 {
            bail!("ACPI hash file must contain 3 lines (loader, rsdp, tables)");
        }
        let loader_hash = hex::decode(lines[0].trim()).context("invalid loader hash hex")?;
        let rsdp_hash = hex::decode(lines[1].trim()).context("invalid rsdp hash hex")?;
        let tables_hash = hex::decode(lines[2].trim()).context("invalid tables hash hex")?;
        if loader_hash.len() != 48 || rsdp_hash.len() != 48 || tables_hash.len() != 48 {
            bail!(
                "ACPI hashes must be 48 bytes (SHA-384), got {}/{}/{}",
                loader_hash.len(),
                rsdp_hash.len(),
                tables_hash.len()
            );
        }
        Ok(Self {
            loader_hash,
            rsdp_hash,
            tables_hash,
        })
    }
}

/// Boot variable data for RTMR[0] measurement.
///
/// EV_EFI_VARIABLE_BOOT events: the digest is SHA384 of the raw variable
/// value (NOT the UEFI_VARIABLE_DATA wrapper that appears in the event log).
///
/// For UKI boot on q35, there are 4 boot variables in order:
///   1. BootOrder  (array of UINT16 boot option numbers)
///   2. Boot0001   (first in boot order — the disk device)
///   3. Boot0000   (UiApp — OVMF internal)
///   4. Boot0002   (EFI Internal Shell — OVMF internal)
pub struct BootVars {
    /// Raw variable values in measurement order.
    pub entries: Vec<Vec<u8>>,
}

impl BootVars {
    /// Load boot variables from a directory containing BootOrder.bin, Boot0000.bin, etc.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let boot_order_path = dir.join("BootOrder.bin");
        let boot_order = fs::read(&boot_order_path)
            .with_context(|| format!("failed to read {}", boot_order_path.display()))?;

        if boot_order.len() % 2 != 0 {
            bail!("BootOrder data length must be even (array of UINT16s)");
        }

        let mut entries = vec![boot_order.clone()];

        // Parse boot order to determine which Boot#### vars to load
        for chunk in boot_order.chunks(2) {
            let num = u16::from_le_bytes([chunk[0], chunk[1]]);
            let filename = format!("Boot{:04X}.bin", num);
            let path = dir.join(&filename);
            let data =
                fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
            entries.push(data);
        }

        Ok(Self { entries })
    }

    /// Extract boot variables from a CCEL event log.
    ///
    /// Finds EV_EFI_VARIABLE_BOOT events (type 0x80000002) targeting RTMR[0]
    /// and extracts the raw variable data from each.
    pub fn extract_from_ccel(ccel_data: &[u8]) -> Result<Self> {
        let events = crate::ccel::parse_ccel(ccel_data)?;
        let mut entries = Vec::new();

        for ev in &events {
            if ev.event_type == 0x80000002 && ev.mr_index == 1 {
                if let Some((_name, data)) = crate::ccel::parse_uefi_variable_data(&ev.event_data) {
                    entries.push(data.to_vec());
                }
            }
        }

        if entries.is_empty() {
            bail!("No EV_EFI_VARIABLE_BOOT events found in CCEL");
        }

        Ok(Self { entries })
    }
}

const PAGE_SIZE: u64 = 0x1000;
const MR_EXTEND_GRANULARITY: usize = 0x100;

const ATTRIBUTE_MR_EXTEND: u32 = 0x0000_0001;
const ATTRIBUTE_PAGE_AUG: u32 = 0x0000_0002;

const TDVF_SECTION_TD_CFV: u32 = 0x01;
const TDVF_SECTION_TD_HOB: u32 = 0x02;
const TDVF_SECTION_TEMP_MEM: u32 = 0x03;

// INVARIANT: Must match OVMF's resource attribute for TD-HOB memory descriptors.
// EFI_RESOURCE_ATTRIBUTE_PRESENT | EFI_RESOURCE_ATTRIBUTE_INITIALIZED | EFI_RESOURCE_ATTRIBUTE_TESTED
const HOB_RESOURCE_ATTRIBUTE: u32 = 0x0000_0007;

#[derive(Debug)]
struct TdvfSection {
    data_offset: u32,
    raw_data_size: u32,
    memory_address: u64,
    memory_data_size: u64,
    sec_type: u32,
    attributes: u32,
}

pub struct Tdvf<'a> {
    fw: &'a [u8],
    sections: Vec<TdvfSection>,
}

fn encode_guid(guid_str: &str) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(16);
    let atoms: Vec<&str> = guid_str.split('-').collect();
    if atoms.len() != 5 {
        bail!("Invalid GUID format: {}", guid_str);
    }
    for (idx, atom) in atoms.iter().enumerate() {
        let raw = hex::decode(atom).context("Failed to decode hex in GUID")?;
        if idx <= 2 {
            for i in (0..raw.len()).rev() {
                data.push(raw[i]);
            }
        } else {
            data.extend_from_slice(&raw);
        }
    }
    Ok(data)
}

/// Measure an EFI variable event (for Secure Boot vars in RTMR[0]).
fn measure_efi_variable(
    vendor_guid: &str,
    var_name: &str,
    var_data: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    data.extend_from_slice(&encode_guid(vendor_guid)?);
    // UnicodeNameLength: count of UTF-16 code units (not UTF-8 bytes)
    let utf16_len =
        u64::try_from(var_name.encode_utf16().count()).context("var name length overflow")?;
    data.extend_from_slice(&utf16_len.to_le_bytes());

    let data_size =
        u64::try_from(var_data.map_or(0, |d| d.len())).context("var data size overflow")?;
    data.extend_from_slice(&data_size.to_le_bytes());

    // Variable name as UTF-16LE
    let utf16: Vec<u8> = var_name
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    data.extend_from_slice(&utf16);

    if let Some(vd) = var_data {
        data.extend_from_slice(vd);
    }

    Ok(sha384(&data))
}

impl<'a> Tdvf<'a> {
    pub fn parse(fw: &'a [u8]) -> Result<Tdvf<'a>> {
        const TDX_METADATA_OFFSET_GUID: &str = "e47a6535-984a-4798-865e-4685a7bf8ec2";
        const TABLE_FOOTER_GUID: &str = "96b582de-1fb2-45f7-baea-a366c55a082d";
        const BYTES_AFTER_TABLE_FOOTER: usize = 32;
        // Need at least: footer (32) + GUID (16) + table length (2) = 50
        const MIN_FIRMWARE_SIZE: usize = BYTES_AFTER_TABLE_FOOTER + 18;

        if fw.len() < MIN_FIRMWARE_SIZE {
            bail!(
                "Firmware too small: {} bytes (need at least {})",
                fw.len(),
                MIN_FIRMWARE_SIZE
            );
        }

        let offset = fw.len() - BYTES_AFTER_TABLE_FOOTER;
        let encoded_footer_guid = encode_guid(TABLE_FOOTER_GUID)?;
        let guid = &fw[offset - 16..offset];

        if guid != encoded_footer_guid.as_slice() {
            bail!("Invalid TDVF footer GUID");
        }

        let tables_len = usize::from(u16::from_le_bytes(
            fw[offset - 18..offset - 16]
                .try_into()
                .context("reading table length")?,
        ));
        if tables_len == 0 || tables_len > offset - 18 {
            bail!("Invalid TDVF tables length");
        }
        let tables = &fw[offset - 18 - tables_len..offset - 18];
        let mut tbl_offset = tables.len();

        let mut data: Option<&[u8]> = None;
        let encoded_guid = encode_guid(TDX_METADATA_OFFSET_GUID)?;

        loop {
            if tbl_offset < 18 {
                break;
            }
            let tbl_guid = &tables[tbl_offset - 16..tbl_offset];
            let entry_len = usize::from(u16::from_le_bytes(
                tables[tbl_offset - 18..tbl_offset - 16]
                    .try_into()
                    .context("reading entry length")?,
            ));
            if entry_len > tbl_offset - 18 {
                bail!("Invalid TDVF entry length");
            }
            if tbl_guid == encoded_guid.as_slice() {
                data = Some(&tables[tbl_offset - 18 - entry_len..tbl_offset - 18]);
                break;
            }
            tbl_offset -= entry_len;
        }

        let data = data.context("Missing TDVF metadata")?;

        if data.len() < 4 {
            bail!("TDVF metadata entry too short");
        }
        let meta_offset_raw_u32 = u32::from_le_bytes(
            data[data.len() - 4..]
                .try_into()
                .context("reading metadata offset")?,
        );
        let meta_offset_raw = usize::try_from(meta_offset_raw_u32)
            .context("TDVF metadata offset exceeds addressable range")?;
        if meta_offset_raw > fw.len() {
            bail!(
                "TDVF metadata offset ({}) exceeds firmware size ({})",
                meta_offset_raw,
                fw.len()
            );
        }
        let tdvf_meta_offset = fw.len() - meta_offset_raw;
        if tdvf_meta_offset + 16 > fw.len() {
            bail!("TDVF metadata descriptor extends beyond firmware");
        }
        let tdvf_meta_desc = &fw[tdvf_meta_offset..tdvf_meta_offset + 16];

        if &tdvf_meta_desc[..4] != b"TDVF" {
            bail!("Invalid TDVF descriptor");
        }
        let version = u32::from_le_bytes(
            tdvf_meta_desc[8..12]
                .try_into()
                .context("reading TDVF version")?,
        );
        if version != 1 {
            bail!("Unsupported TDVF version: {}", version);
        }
        let num_sections = usize::try_from(u32::from_le_bytes(
            tdvf_meta_desc[12..16]
                .try_into()
                .context("reading section count")?,
        ))
        .context("section count exceeds addressable range")?;

        let sections_end = tdvf_meta_offset
            .checked_add(16)
            .and_then(|v| v.checked_add(num_sections.checked_mul(32)?))
            .context("TDVF section table size overflow")?;
        if sections_end > fw.len() {
            bail!(
                "TDVF section table ({} sections) extends beyond firmware at offset 0x{:x}",
                num_sections,
                tdvf_meta_offset
            );
        }

        let mut meta = Tdvf {
            fw,
            sections: Vec::new(),
        };

        for i in 0..num_sections {
            let sec_off = tdvf_meta_offset + 16 + 32 * i;
            let sec = &fw[sec_off..sec_off + 32];
            let s = TdvfSection {
                data_offset: u32::from_le_bytes(
                    sec[0..4].try_into().context("reading data_offset")?,
                ),
                raw_data_size: u32::from_le_bytes(
                    sec[4..8].try_into().context("reading raw_data_size")?,
                ),
                memory_address: u64::from_le_bytes(
                    sec[8..16].try_into().context("reading memory_address")?,
                ),
                memory_data_size: u64::from_le_bytes(
                    sec[16..24].try_into().context("reading memory_data_size")?,
                ),
                sec_type: u32::from_le_bytes(sec[24..28].try_into().context("reading sec_type")?),
                attributes: u32::from_le_bytes(
                    sec[28..32].try_into().context("reading attributes")?,
                ),
            };

            if s.memory_address % PAGE_SIZE != 0 {
                bail!("Section {} memory address not page-aligned", i);
            }
            if s.memory_data_size < u64::from(s.raw_data_size) {
                bail!("Section {} memory_data_size < raw_data_size", i);
            }
            if s.memory_data_size % PAGE_SIZE != 0 {
                bail!("Section {} memory_data_size not page-aligned", i);
            }
            // Validate section data is within firmware bounds
            let fw_len_u64 = u64::try_from(fw.len()).context("firmware length overflow")?;
            let data_end = u64::from(s.data_offset)
                .checked_add(u64::from(s.raw_data_size))
                .context("section data_offset + raw_data_size overflow")?;
            if data_end > fw_len_u64 {
                bail!(
                    "Section {} data (offset 0x{:x}, size 0x{:x}) extends beyond firmware (0x{:x})",
                    i,
                    s.data_offset,
                    s.raw_data_size,
                    fw.len()
                );
            }
            // For MR_EXTEND sections, the loop walks memory_data_size worth of data
            if s.attributes & ATTRIBUTE_MR_EXTEND != 0 {
                let extend_end = u64::from(s.data_offset)
                    .checked_add(s.memory_data_size)
                    .context("section data_offset + memory_data_size overflow")?;
                if extend_end > fw_len_u64 {
                    bail!(
                        "Section {} MR_EXTEND region (offset 0x{:x}, size 0x{:x}) extends beyond firmware (0x{:x})",
                        i,
                        s.data_offset,
                        s.memory_data_size,
                        fw.len()
                    );
                }
            }

            meta.sections.push(s);
        }

        Ok(meta)
    }

    /// Compute MRTD by simulating the TDX Module's build-time measurement.
    ///
    /// For each firmware section:
    ///   - MEM.PAGE.ADD: 128-byte record with GPA
    ///   - MR.EXTEND: 128-byte record + 256 bytes of page data (per chunk)
    /// MRTD = SHA384(all records concatenated)
    pub fn mrtd(&self) -> Result<Vec<u8>> {
        let mut h = Sha384::new();

        for s in &self.sections {
            let num_pages = s.memory_data_size / PAGE_SIZE;

            for page in 0..num_pages {
                // MEM.PAGE.ADD
                if s.attributes & ATTRIBUTE_PAGE_AUG == 0 {
                    let mut buf = [0u8; 128];
                    buf[..12].copy_from_slice(b"MEM.PAGE.ADD");
                    let gpa = s.memory_address + page * PAGE_SIZE;
                    buf[16..24].copy_from_slice(&gpa.to_le_bytes());
                    h.update(buf);
                }

                // MR.EXTEND
                if s.attributes & ATTRIBUTE_MR_EXTEND != 0 {
                    let chunks_per_page = PAGE_SIZE as usize / MR_EXTEND_GRANULARITY;
                    for i in 0..chunks_per_page {
                        let mut buf = [0u8; 128];
                        buf[..9].copy_from_slice(b"MR.EXTEND");
                        let gpa = s.memory_address
                            + page * PAGE_SIZE
                            + (i * MR_EXTEND_GRANULARITY) as u64;
                        buf[16..24].copy_from_slice(&gpa.to_le_bytes());
                        h.update(buf);

                        let chunk_offset =
                            usize::try_from(u64::from(s.data_offset) + page * PAGE_SIZE)
                                .context("MR.EXTEND chunk offset overflow")?
                                + i * MR_EXTEND_GRANULARITY;
                        h.update(&self.fw[chunk_offset..chunk_offset + MR_EXTEND_GRANULARITY]);
                    }
                }
            }
        }

        Ok(h.finalize().to_vec())
    }

    /// Compute RTMR[0] for UKI boot.
    ///
    /// Full event log (15 events):
    ///   1.  TD-HOB hash
    ///   2.  CFV hash
    ///   3.  SecureBoot variable (EV_EFI_VARIABLE_DRIVER_CONFIG)
    ///   4.  PK variable
    ///   5.  KEK variable
    ///   6.  db variable
    ///   7.  dbx variable
    ///   8.  EV_SEPARATOR
    ///   9.  ACPI table-loader (etc/table-loader)
    ///  10.  ACPI RSDP (etc/acpi/rsdp)
    ///  11.  ACPI tables (etc/acpi/tables)
    ///  12.  BootOrder (EV_EFI_VARIABLE_BOOT)
    ///  13.  Boot0001 (EV_EFI_VARIABLE_BOOT)
    ///  14.  Boot0000 (EV_EFI_VARIABLE_BOOT)
    ///  15.  Boot0002 (EV_EFI_VARIABLE_BOOT)
    ///
    /// Returns (rtmr0_value, event_count) where event_count indicates
    /// how many of the 15 events were included.
    pub fn rtmr0(
        &self,
        memory_size: u64,
        acpi: Option<&AcpiHashes>,
        boot_vars: Option<&BootVars>,
    ) -> Result<(Vec<u8>, usize)> {
        let td_hob_hash = self.measure_td_hob(memory_size)?;
        let cfv_hash = self.measure_cfv()?;

        // EFI variables (Secure Boot not enrolled = None data)
        let mut rtmr0_log = vec![
            td_hob_hash,
            cfv_hash,
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "SecureBoot", None)?,
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "PK", None)?,
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "KEK", None)?,
            measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "db", None)?,
            measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "dbx", None)?,
            sha384(&[0x00, 0x00, 0x00, 0x00]), // Separator
        ];

        // ACPI table measurements (events 9-11)
        if let Some(acpi) = acpi {
            rtmr0_log.push(acpi.loader_hash.clone());
            rtmr0_log.push(acpi.rsdp_hash.clone());
            rtmr0_log.push(acpi.tables_hash.clone());
        }

        // Boot variable measurements (events 12-15)
        // DEVIATION: TCG PFP spec says EV_EFI_VARIABLE_BOOT digest = SHA384(UEFI_VARIABLE_DATA).
        // OVMF's CC/TDX implementation uses SHA384(raw_variable_value) instead.
        // Verified against hardware — see PROGRESS.md.
        if let Some(bv) = boot_vars {
            for entry in &bv.entries {
                rtmr0_log.push(sha384(entry));
            }
        }

        let count = rtmr0_log.len();
        let mut mr = vec![0u8; 48];
        for entry in &rtmr0_log {
            mr = rtmr_extend(&mr, entry);
        }

        Ok((mr, count))
    }

    fn measure_td_hob(&self, memory_size: u64) -> Result<Vec<u8>> {
        let mut memory_acceptor = MemoryAcceptor::new(0, memory_size)?;
        let mut td_hob = Vec::new();

        let mut td_hob_base_addr = 0x809000u64;
        for s in &self.sections {
            if matches!(s.sec_type, TDVF_SECTION_TD_HOB | TDVF_SECTION_TEMP_MEM) {
                let accept_end = s
                    .memory_address
                    .checked_add(s.memory_data_size)
                    .context("section memory_address + memory_data_size overflow")?;
                memory_acceptor.accept(s.memory_address, accept_end);
            }
            if s.sec_type == TDVF_SECTION_TD_HOB {
                td_hob_base_addr = s.memory_address;
            }
        }

        // PHIT HOB (EFI_HOB_HANDOFF_INFO_TABLE)
        td_hob.extend_from_slice(&[0x01, 0x00]); // HobType = PHIT
        td_hob.extend_from_slice(&56u16.to_le_bytes()); // HobLength
        td_hob.extend_from_slice(&[0u8; 4]); // Reserved
        td_hob.extend_from_slice(&9u32.to_le_bytes()); // Version
        td_hob.extend_from_slice(&[0u8; 4]); // BootMode
        td_hob.extend_from_slice(&[0u8; 8]); // EfiMemoryTop
        td_hob.extend_from_slice(&[0u8; 8]); // EfiMemoryBottom
        td_hob.extend_from_slice(&[0u8; 8]); // EfiFreeMemoryTop
        td_hob.extend_from_slice(&[0u8; 8]); // EfiFreeMemoryBottom
        td_hob.extend_from_slice(&[0u8; 8]); // EfiEndOfHobList (placeholder)

        let mut add_resource_hob = |resource_type: u8, start: u64, length: u64| {
            td_hob.extend_from_slice(&[0x03, 0x00]); // HobType = RESOURCE_DESCRIPTOR
            td_hob.extend_from_slice(&48u16.to_le_bytes()); // HobLength
            td_hob.extend_from_slice(&[0u8; 4]); // Reserved
            td_hob.extend_from_slice(&[0u8; 16]); // Owner
            td_hob.extend_from_slice(&resource_type.to_le_bytes());
            td_hob.extend_from_slice(&[0u8; 3]); // Padding
            td_hob.extend_from_slice(&HOB_RESOURCE_ATTRIBUTE.to_le_bytes());
            td_hob.extend_from_slice(&start.to_le_bytes());
            td_hob.extend_from_slice(&length.to_le_bytes());
        };

        let &(_, last_start, last_end) = memory_acceptor
            .ranges
            .last()
            .context("MemoryAcceptor has no ranges")?;

        for &(accepted, start, end) in &memory_acceptor.ranges[..memory_acceptor.ranges.len() - 1] {
            if accepted {
                add_resource_hob(0x00, start, end - start); // EFI_RESOURCE_SYSTEM_MEMORY
            } else {
                add_resource_hob(0x07, start, end - start); // EFI_RESOURCE_MEMORY_UNACCEPTED
            }
        }

        if memory_size >= 0xB000_0000 {
            // INVARIANT: last_start < 0x8000_0000 for standard q35 TDX layouts.
            // Firmware sections occupy low memory, leaving the large region above them.
            if last_start >= 0x8000_0000 {
                bail!(
                    "TD HOB: last memory range starts at 0x{:x}, expected below 0x80000000",
                    last_start
                );
            }
            if last_end < 0x8000_0000 {
                bail!(
                    "TD HOB: last memory range ends at 0x{:x}, expected above 0x80000000",
                    last_end
                );
            }
            add_resource_hob(0x07, last_start, 0x8000_0000u64 - last_start);
            add_resource_hob(0x07, 0x1_0000_0000, last_end - 0x8000_0000u64);
        } else {
            add_resource_hob(0x07, last_start, last_end - last_start);
        }

        // Fix up EfiEndOfHobList
        let end_of_hob_list =
            td_hob_base_addr + u64::try_from(td_hob.len()).context("TD HOB size overflow")? + 8;
        td_hob[48..56].copy_from_slice(&end_of_hob_list.to_le_bytes());

        Ok(sha384(&td_hob))
    }

    fn measure_cfv(&self) -> Result<Vec<u8>> {
        for section in &self.sections {
            if section.sec_type == TDVF_SECTION_TD_CFV {
                let start =
                    usize::try_from(section.data_offset).context("CFV data_offset overflow")?;
                let end = start
                    .checked_add(
                        usize::try_from(section.raw_data_size)
                            .context("CFV raw_data_size overflow")?,
                    )
                    .context("CFV offset + size overflow")?;
                if end > self.fw.len() {
                    bail!("CFV section extends beyond firmware");
                }
                return Ok(sha384(&self.fw[start..end]));
            }
        }
        bail!("CFV section not found in firmware")
    }
}

struct MemoryAcceptor {
    ranges: Vec<(bool, u64, u64)>,
}

impl MemoryAcceptor {
    fn new(start: u64, size: u64) -> Result<Self> {
        let end = start
            .checked_add(size)
            .context("MemoryAcceptor: start + size overflow")?;
        Ok(Self {
            ranges: vec![(false, start, end)],
        })
    }

    fn accept(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let mut new_ranges = Vec::new();
        for &(is_accepted, range_start, range_end) in &self.ranges {
            if is_accepted || range_end <= start || range_start >= end {
                new_ranges.push((is_accepted, range_start, range_end));
            } else {
                if range_start < start {
                    new_ranges.push((false, range_start, start));
                }
                if range_end > end {
                    new_ranges.push((false, end, range_end));
                }
            }
        }
        new_ranges.push((true, start, end));
        new_ranges.sort_by_key(|&(_, s, _)| s);
        debug_assert!(
            new_ranges.windows(2).all(|w| w[0].2 <= w[1].1),
            "MemoryAcceptor: overlapping ranges detected after accept({:#x}, {:#x})",
            start,
            end,
        );
        self.ranges = new_ranges;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_guid_standard() {
        // Known GUID: EFI Global Variable
        let result = encode_guid("8BE4DF61-93CA-11D2-AA0D-00E098032B8C").unwrap();
        assert_eq!(result.len(), 16);
        // First 3 groups reversed (little-endian), last 2 groups big-endian
        assert_eq!(result[0], 0x61); // DF61 reversed => 61 DF
        assert_eq!(result[1], 0xDF);
        assert_eq!(result[2], 0xE4);
        assert_eq!(result[3], 0x8B);
    }

    #[test]
    fn test_encode_guid_invalid_format() {
        assert!(encode_guid("not-a-guid").is_err());
        assert!(encode_guid("").is_err());
        assert!(encode_guid("8BE4DF61-93CA-11D2-AA0D").is_err()); // only 4 parts
    }

    #[test]
    fn test_encode_guid_roundtrip() {
        // Two different GUIDs should produce different encodings
        let g1 = encode_guid("8BE4DF61-93CA-11D2-AA0D-00E098032B8C").unwrap();
        let g2 = encode_guid("D719B2CB-3D3A-4596-A3BC-DAD00E67656F").unwrap();
        assert_ne!(g1, g2);
    }

    #[test]
    fn test_measure_efi_variable_no_data() {
        let result =
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "SecureBoot", None)
                .unwrap();
        assert_eq!(result.len(), 48); // SHA-384
    }

    #[test]
    fn test_measure_efi_variable_with_data() {
        let result = measure_efi_variable(
            "8BE4DF61-93CA-11D2-AA0D-00E098032B8C",
            "SecureBoot",
            Some(&[0x01]),
        )
        .unwrap();
        assert_eq!(result.len(), 48);

        // With different data, the hash should differ
        let result2 = measure_efi_variable(
            "8BE4DF61-93CA-11D2-AA0D-00E098032B8C",
            "SecureBoot",
            Some(&[0x00]),
        )
        .unwrap();
        assert_ne!(result, result2);
    }

    #[test]
    fn test_measure_efi_variable_different_names() {
        let h1 = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "PK", None).unwrap();
        let h2 = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "KEK", None).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_measure_efi_variable_deterministic() {
        let h1 = measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "db", None).unwrap();
        let h2 = measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "db", None).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_memory_acceptor_new() {
        let ma = MemoryAcceptor::new(0, 0x1_0000_0000).unwrap();
        assert_eq!(ma.ranges.len(), 1);
        assert_eq!(ma.ranges[0], (false, 0, 0x1_0000_0000));
    }

    #[test]
    fn test_memory_acceptor_overflow() {
        let result = MemoryAcceptor::new(u64::MAX, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_memory_acceptor_accept_middle() {
        let mut ma = MemoryAcceptor::new(0, 0x1000).unwrap();
        ma.accept(0x400, 0x800);
        // Should split into: [0,0x400) unaccepted, [0x400,0x800) accepted, [0x800,0x1000) unaccepted
        assert_eq!(ma.ranges.len(), 3);
        assert_eq!(ma.ranges[0], (false, 0, 0x400));
        assert_eq!(ma.ranges[1], (true, 0x400, 0x800));
        assert_eq!(ma.ranges[2], (false, 0x800, 0x1000));
    }

    #[test]
    fn test_memory_acceptor_accept_start() {
        let mut ma = MemoryAcceptor::new(0, 0x1000).unwrap();
        ma.accept(0, 0x400);
        assert_eq!(ma.ranges.len(), 2);
        assert_eq!(ma.ranges[0], (true, 0, 0x400));
        assert_eq!(ma.ranges[1], (false, 0x400, 0x1000));
    }

    #[test]
    fn test_memory_acceptor_accept_end() {
        let mut ma = MemoryAcceptor::new(0, 0x1000).unwrap();
        ma.accept(0x800, 0x1000);
        assert_eq!(ma.ranges.len(), 2);
        assert_eq!(ma.ranges[0], (false, 0, 0x800));
        assert_eq!(ma.ranges[1], (true, 0x800, 0x1000));
    }

    #[test]
    fn test_memory_acceptor_noop_on_empty_range() {
        let mut ma = MemoryAcceptor::new(0, 0x1000).unwrap();
        ma.accept(0x500, 0x500); // start == end
        assert_eq!(ma.ranges.len(), 1);
        ma.accept(0x800, 0x400); // start > end
        assert_eq!(ma.ranges.len(), 1);
    }

    #[test]
    fn test_memory_acceptor_multiple_accepts() {
        let mut ma = MemoryAcceptor::new(0, 0x2000).unwrap();
        ma.accept(0x0, 0x800);
        ma.accept(0x1000, 0x1800);
        // Should have: [0,0x800) accepted, [0x800,0x1000) unaccepted,
        //              [0x1000,0x1800) accepted, [0x1800,0x2000) unaccepted
        assert_eq!(ma.ranges.len(), 4);
        assert!(ma.ranges[0].0); // accepted
        assert!(!ma.ranges[1].0); // unaccepted
        assert!(ma.ranges[2].0); // accepted
        assert!(!ma.ranges[3].0); // unaccepted
    }

    #[test]
    fn test_tdvf_parse_rejects_small_firmware() {
        let fw = vec![0u8; 16];
        assert!(Tdvf::parse(&fw).is_err());
    }

    #[test]
    fn test_tdvf_parse_rejects_bad_footer_guid() {
        let fw = vec![0u8; 256];
        let result = Tdvf::parse(&fw);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("GUID"));
    }

    /// Pin the exact Secure Boot variable digests used in RTMR[0] events 3-7.
    #[test]
    fn test_efi_variable_golden_values() {
        let sb = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "SecureBoot", None)
            .unwrap();
        assert_eq!(
            hex::encode(&sb),
            "9dc3a1f80bcec915391dcda5ffbb15e7419f77eab462bbf72b42166fb70d50325e37b36f93537a863769bcf9bedae6fb",
        );

        let pk = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "PK", None).unwrap();
        assert_eq!(
            hex::encode(&pk),
            "6f2e3cbc14f9def86980f5f66fd85e99d63e69a73014ed8a5633ce56eca5b64b692108c56110e22acadcef58c3250f1b",
        );

        let kek =
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "KEK", None).unwrap();
        assert_eq!(
            hex::encode(&kek),
            "d607c0efb41c0d757d69bca0615c3a9ac0b1db06c557d992e906c6b7dee40e0e031640c7bfd7bcd35844ef9edeadc6f9",
        );

        let db = measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "db", None).unwrap();
        assert_eq!(
            hex::encode(&db),
            "08a74f8963b337acb6c93682f934496373679dd26af1089cb4eaf0c30cf260a12e814856385ab8843e56a9acea19e127",
        );

        let dbx =
            measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "dbx", None).unwrap();
        assert_eq!(
            hex::encode(&dbx),
            "18cc6e01f0c6ea99aa23f8a280423e94ad81d96d0aeb5180504fc0f7a40cb3619dd39bd6a95ec1680a86ed6ab0f9828d",
        );
    }

    /// Pin the RTMR[0] Secure Boot chain (events 3-8, sans td_hob/cfv).
    #[test]
    fn test_rtmr0_secboot_chain_golden_value() {
        use crate::rtmr::rtmr_extend;

        let sb = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "SecureBoot", None)
            .unwrap();
        let pk = measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "PK", None).unwrap();
        let kek =
            measure_efi_variable("8BE4DF61-93CA-11D2-AA0D-00E098032B8C", "KEK", None).unwrap();
        let db = measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "db", None).unwrap();
        let dbx =
            measure_efi_variable("D719B2CB-3D3A-4596-A3BC-DAD00E67656F", "dbx", None).unwrap();
        let sep = crate::rtmr::sha384(&[0x00, 0x00, 0x00, 0x00]);

        let mut mr = vec![0u8; 48];
        for digest in [&sb, &pk, &kek, &db, &dbx, &sep] {
            mr = rtmr_extend(&mr, digest);
        }
        assert_eq!(
            hex::encode(&mr),
            "384a798e98b5369727295c5b2889e1e015c65b3add6d78dce4e457c164865ee2af7b6d53ac7cbcd092eaf5fa1a76ea18",
        );
    }
}
