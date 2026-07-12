# Changelog

All notable changes to steep are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
the policy in [docs/VERSIONING.md](docs/VERSIONING.md) — in particular,
entries call out changes that **alter measurements** of otherwise-identical
build configs, since those invalidate published reference values.

## [Unreleased]

### Changed

- `steep push` now defaults to the registry `ghcr.io/confidential-dot-ai/steep`
  (previously `docker.io/confidentialai`) and addresses images as
  `<registry>:<tag>`, with the tag defaulting to the pushed directory's
  basename — `steep push output/base` publishes
  `ghcr.io/confidential-dot-ai/steep:base`. The `--name` flag is removed.
- `steep pull` now takes a full image reference and an optional destination
  directory (`steep pull ghcr.io/confidential-dot-ai/steep:base [DIR]`),
  defaulting to `output/<tag>`; the `--registry` and `--tag` flags are
  removed.
- CI now publishes the base image as `ghcr.io/confidential-dot-ai/steep:base`
  (plus a pinned `base-<short-sha>` tag) instead of
  `ghcr.io/confidential-dot-ai/steep/base:latest`.

## [0.1.0] — 2026-07-09

Initial public release.

### Added

- `steep build` — reproducible dm-verity + UKI image pipeline on mkosi, with
  cloud-init/`--extra`/`--package`/`--script` content injection.
- Hardened pinned guest kernel (Linux 6.16.x) with fragment-based
  configuration and a committed resolved-config snapshot lockfile.
- AMD SEV-SNP support: per-SMP IGVM generation and offline launch-digest
  computation (`crates/igvm-tools`), QEMU+KVM semantics.
- Intel TDX support: offline MRTD/RTMR computation and attestation
  verification tooling (`crates/tdx-measure`).
- Trusted-DSDT override shipping an audited DSDT in the measured initrd
  (TDX BadAML mitigation; enables topology-invariant TDX manifests).
- Manifest schema v3: `snp_variants[]` + singleton `tdx` block.
- `steep run` (SNP → KVM → emulated tier autodetection, port forwarding,
  ephemeral encrypted scratch disks), `steep igvm`,
  `steep push` / `steep pull` (OCI via oras, optional KubeVirt CDI layout),
  and `steep kernel` (a hidden helper subcommand for iterating on kernel
  config out-of-band; `steep build` invokes the same machinery).
- CI-published base image at `ghcr.io/confidential-dot-ai/steep/base`.

[Unreleased]: https://github.com/confidential-dot-ai/steep/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/confidential-dot-ai/steep/releases/tag/v0.1.0
