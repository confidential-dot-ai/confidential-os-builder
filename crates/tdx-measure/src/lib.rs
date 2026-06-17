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
