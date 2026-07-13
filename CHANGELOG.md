# Changelog

All notable changes to steep are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
the policy in [docs/VERSIONING.md](docs/VERSIONING.md) — in particular,
entries call out changes that **alter measurements** of otherwise-identical
build configs, since those invalidate published reference values.

## [Unreleased]

## [0.1.0] — 2026-07-13

Initial public release.

### Added

- `steep build` — reproducible dm-verity + UKI image pipeline on mkosi, with
  cloud-init/`--extra`/`--package`/`--script` content injection.
- Hardened pinned guest kernel (Linux 6.16.x) with fragment-based
  configuration and a committed resolved-config snapshot lockfile.
- Manifest schema v3, supporting images for AMD SEV-SNP and Intel TDX.
- Intel TDX support: offline MRTD/RTMR computation and attestation
  verification tooling (`crates/tdx-measure`).
- AMD SEV-SNP support: per-SMP IGVM generation and offline launch-digest
  computation (`crates/igvm-tools`), QEMU+KVM semantics.
- `steep run` (SNP → KVM → emulated tier autodetection, port forwarding,
  ephemeral encrypted scratch disks)
- `steep push` / `steep pull` (OCI via oras)
- CI publishes base image as `ghcr.io/confidential-dot-ai/steep:base`

[Unreleased]: https://github.com/confidential-dot-ai/steep/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/confidential-dot-ai/steep/releases/tag/v0.1.0
