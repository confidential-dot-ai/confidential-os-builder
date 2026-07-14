# Changelog

All notable changes to Confidential OS Builder are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
the policy in [docs/VERSIONING.md](docs/VERSIONING.md) — in particular,
entries call out changes that **alter measurements** of otherwise-identical
build configs, since those invalidate published reference values.

## [Unreleased]

## [0.2.0] — 2026-07-13

**Steep is renamed to ConfidentialOS Builder**

Breaking changes for existing users:
  - Binary renamed: `steep` is now `confos`
  - Repository moved to [confidential-dot-ai/confidential-os-builder](https://github.com/confidential-dot-ai/confidential-os-builder)
  - Crate renamed to `confidential-os-builder`
  - Env vars renamed: `STEEP_QEMU_BIN`, `STEEP_FIRMWARE`, `STEEP_TDX_FIRMWARE`,
    `STEEP_OCI_REGISTRY` → `CONFOS_QEMU_BIN`, `CONFOS_FIRMWARE`,
    `CONFOS_TDX_FIRMWARE`, `CONFOS_OCI_REGISTRY`
  - Default registry and published base images move to
    `ghcr.io/confidential-dot-ai/confidential-os-builder`; existing
    `ghcr.io/confidential-dot-ai/steep` tags remain but are frozen.
  - OCI artifact media types updated:
    - `application/vnd.steep.image.v1` →
    `application/vnd.confos.image.v1`
  - **Changes measurements.** The baked-in guest hostname and the cloud-init
    seed (`instance-id`, `local-hostname`) are renamed `steep` → `confos`,
    along with the kernel build stamps (`KBUILD_BUILD_USER`/`KBUILD_BUILD_HOST`)
    and comments in measured image-input files, so every published 0.1.x
    measurement is invalid for 0.2.0 builds.

## [0.1.1] — 2026-07-13

- Add direct-kernel boot mode for running inside Kata Containers (#42)
- Add workload measurement hook to TDX attestations and verifications (#43)

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

[Unreleased]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/confidential-dot-ai/confidential-os-builder/releases/tag/v0.1.0
