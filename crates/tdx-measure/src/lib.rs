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
pub mod esp;
pub mod pe;
pub mod rtmr;
pub mod tdvf;

/// A topology-invariant TDX measurement bundle for a UKI boot. Carries
/// MRTD + RTMR[1] + RTMR[2] — the three registers that are stable across
/// vCPU and memory configurations under confos's confidential VM model.
///
/// **RTMR[0] is deliberately absent.** It depends on TD-HOB and
/// VMM-supplied ACPI tables, both of which vary at runtime. Callers that
/// need to attest RTMR[0] must replay the CCEL event log against the
/// runtime quote (see `tdx_measure::ccel`).
///
/// All fields are hex-encoded SHA-384 digests (96 lowercase hex chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UkiMeasurement {
    pub mrtd: String,
    pub rtmr1: String,
    pub rtmr2: String,
}

/// Compute the **topology-invariant subset** of TDX measurements for a
/// UKI boot: MRTD + RTMR[1] + RTMR[2]. RTMR[0] is intentionally omitted
/// — see [`UkiMeasurement`] for the rationale and the CCEL-replay path
/// for runtime RTMR[0] attestation.
///
/// The function name is suffixed `_topology_invariant` to make it
/// obvious at every call site that the result is a three-register
/// bundle, not a full quote. A future caller looking for "the
/// measurement" and writing a policy gate would otherwise silently get
/// a 3-of-4 comparison and accept any RTMR[0] without realizing it.
///
/// `disk_image` is optional. When present, RTMR[1] includes the GPT
/// header event the firmware emits before launching the UKI. When
/// absent, the GPT event is skipped and the resulting RTMR[1] will not
/// match a real disk-boot CCEL — useful only for unit tests or
/// `-kernel`-style direct boots.
pub fn measure_uki_topology_invariant(
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
    fn measure_uki_topology_invariant_rejects_empty_firmware() {
        let result = measure_uki_topology_invariant(&[], &[], None);
        assert!(result.is_err());
    }

    /// Sanity check: measure_uki fails on bogus firmware bytes that won't
    /// match the TDVF table footer GUID, AND the failure is rooted in
    /// the TDVF parse — not a PE-parse short-circuit on a too-small UKI.
    ///
    /// Original version of this test used a 64-byte UKI buffer. A 64-byte
    /// UKI is too short for any PE/COFF parser to validate (DOS header
    /// alone is 64 bytes); any future TDVF-parse regression that silently
    /// returns zeros would still be caught by the PE-parse error. By
    /// passing a UKI long enough to clear the PE parser's headers
    /// length check we ensure this test fails for the *TDVF* reason
    /// it claims to fail for.
    #[test]
    fn measure_uki_topology_invariant_rejects_garbage_firmware() {
        // 4KiB of zeros; no TDVF footer GUID present.
        let firmware = vec![0u8; 4096];
        // 4KiB UKI buffer with a plausible-looking DOS magic so the PE
        // parser doesn't reject on the first byte. The error must come
        // from TDVF parsing, not PE parsing.
        let mut uki = vec![0u8; 4096];
        uki[0..2].copy_from_slice(b"MZ");
        let result = measure_uki_topology_invariant(&firmware, &uki, None);
        let err = result.expect_err("garbage firmware must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("tdvf") || msg.to_lowercase().contains("firmware"),
            "rejection should originate from TDVF/firmware parse, got: {msg}"
        );
    }
}
