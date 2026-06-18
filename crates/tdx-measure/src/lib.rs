//! Offline TDX measurement computation and attestation verification.
//!
//! Computes the expected TDX measurement registers for a confidential
//! VM boot, hardware-verified against live Intel TDX hardware:
//!
//! - **MRTD** — the static firmware measurement, computed from TDVF
//!   metadata by simulating the TDX module's `MEM.PAGE.ADD` +
//!   `MR.EXTEND` algorithm.
//! - **RTMR[0]** — the firmware + early-boot events (TD-HOB, CFV,
//!   secure boot variables, ACPI tables, boot variables).
//! - **RTMR[1]** — the UKI PE image identity (Authenticode hash, GPT,
//!   kernel, boot service constants).
//! - **RTMR[2]** — the UKI section measurements (kernel, initrd,
//!   cmdline, osrel, uname, sbat).
//!
//! Also parses CCEL (CC Event Log) blobs and replays them against a
//! TDREPORT for consistency verification.
//!
//! See the `tdx-measure` binary in `main.rs` for the user-facing CLI.
//! Library callers can use the per-module APIs directly:
//!
//! ```ignore
//! let sections = tdx_measure::pe::parse_sections(uki_bytes)?;
//! let rtmr1 = tdx_measure::rtmr::compute_rtmr1_uki(uki_bytes, &sections, Some(disk_bytes))?;
//! ```
//!
//! Hardware verification status is tracked in the source repository's
//! `PROGRESS.md`; the modules below match live TDX hardware exactly
//! for the documented boot flow.

pub mod ccel;
pub mod pe;
pub mod rtmr;
pub mod tdvf;

/// A minimal TDX measurement bundle for a UKI boot — MRTD plus the two
/// RTMR registers that are stable across SMP and memory topologies in
/// steep's confidential VM model.
///
/// Hex-encoded SHA-384 digests (96 lowercase hex chars each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UkiMeasurement {
    pub mrtd: String,
    pub rtmr1: String,
    pub rtmr2: String,
}

/// Compute the three TDX measurements that downstream code (the build
/// manifest, attestation verifiers) actually needs from a steep build:
/// MRTD, RTMR[1], RTMR[2].
///
/// RTMR[0] is intentionally not produced here. It depends on TD-HOB and
/// VMM-supplied ACPI tables, both of which vary with the runtime memory
/// and (for some ACPI fields) vCPU topology. steep handles those at the
/// trust boundary by overriding the DSDT via the initrd, then attesting
/// the override via RTMR[2] / the IGVM launch digest — RTMR[0] itself
/// is left to the runtime CCEL replay path in the attestation API.
///
/// `disk_image` is optional. When present, RTMR[1] includes the GPT
/// header event the firmware emits before launching the UKI. When
/// absent, the GPT event is skipped and the resulting RTMR[1] will not
/// match a real boot's CCEL — useful only for unit tests.
pub fn measure_uki(
    firmware: &[u8],
    uki: &[u8],
    disk_image: Option<&[u8]>,
) -> anyhow::Result<UkiMeasurement> {
    let tdvf = tdvf::Tdvf::parse(firmware)?;
    let mrtd = tdvf.mrtd()?;

    let sections = pe::parse_sections(uki)?;
    let rtmr1 = rtmr::compute_rtmr1_uki(uki, &sections, disk_image)?;
    let (rtmr2, _count) = rtmr::compute_rtmr2_uki(&sections)?;

    Ok(UkiMeasurement {
        mrtd: hex::encode(mrtd),
        rtmr1: hex::encode(rtmr1),
        rtmr2: hex::encode(rtmr2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check: measure_uki fails on an empty firmware buffer (TDVF
    /// parse will reject it), rather than silently returning zeros.
    #[test]
    fn measure_uki_rejects_empty_firmware() {
        let result = measure_uki(&[], &[], None);
        assert!(result.is_err());
    }

    /// Sanity check: measure_uki fails on bogus firmware bytes that won't
    /// match the TDVF table footer GUID.
    #[test]
    fn measure_uki_rejects_garbage_firmware() {
        // 4KiB of zeros; no TDVF footer GUID present.
        let firmware = vec![0u8; 4096];
        let result = measure_uki(&firmware, &[0u8; 64], None);
        assert!(result.is_err());
    }
}
