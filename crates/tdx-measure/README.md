# tdx-measure

> Part of [confos](https://github.com/confidential-dot-ai/confidential-os-builder). AGPL-3.0.

Offline TDX measurement computation and attestation verification.

This crate is the TDX counterpart to `igvm-tools` (SEV-SNP launch
digest). It computes the expected values of the TDX measurement
registers (MRTD, RTMR[1], and RTMR[2]; RTMR[0] additionally requires
dumped ACPI blobs — see `acpi-extract/` below) from the same inputs the
hypervisor uses at launch, so a build system can publish the expected
measurement alongside the artifact for downstream attestation.

The library exposes five modules:

- `tdvf` — parse the TDVF metadata table out of an OVMF/TDVF firmware
  binary, compute MRTD by simulating `MEM.PAGE.ADD` + `MR.EXTEND`, and
  compute RTMR[0]'s 15 firmware-stage events.
- `rtmr` — compute RTMR[1] (UKI PE image identity, GPT, kernel boot
  service constants) and RTMR[2] (UKI section measurements) for a UKI
  boot.
- `pe` — PE/COFF parsing and Authenticode SHA-384 hashing of UKI
  binaries.
- `esp` — read systemd-boot's PE binary out of the FAT32 ESP inside a
  disk image, so RTMR[1] can cover the bootloader actually on disk.
- `ccel` — parse a CCEL (CC Event Log) blob, replay it into RTMRs, and
  extract per-event data such as UEFI variables.

The crate root exposes `measure_uki_topology_invariant()`, the
high-level entry point that returns a `UkiMeasurement`
(MRTD + RTMR[1] + RTMR[2]) — this is what `confos build` calls.

The included `tdx-measure` CLI exposes `measure`, `verify`, `inspect`,
and `extract-platform` subcommands. The CLI is gated behind the `cli`
feature (enabled by default); library users can disable default
features to avoid pulling in `clap` and `serde_json`.

The expected measurements computed by this crate have been verified
against live Intel TDX hardware. The original repository's
`PROGRESS.md` is the authoritative reference for which boot flow has
been validated.

The `acpi-extract/` directory ships a Docker recipe for running QEMU
once to dump the ACPI fw_cfg blobs (`acpi_tables.bin`, `rsdp.bin`,
`table_loader.bin`) that the TDVF RTMR[0] computation needs. Run it
once per firmware + machine config; the outputs are deterministic.
