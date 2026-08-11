# Changelog

All notable changes to Confidential OS Builder are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
the policy in [docs/VERSIONING.md](docs/VERSIONING.md) — in particular,
entries call out changes that **alter measurements** of otherwise-identical
build configs, since those invalidate published reference values.

## [Unreleased]

### Fixed
- **Changes measurements (once); needs a new CI secret.** Kernel builds are
  bit-reproducible: the module-signing trust anchor is now the committed
  public certificate `kernel/module-signing.crt`, built into the system
  keyring via `CONFIG_SYSTEM_TRUSTED_KEYS`, instead of a certificate the
  kernel build generated per run — which made vmlinuz, and every measurement
  downstream of it, differ on each kernel cache miss (#85). The private key
  never enters this repo or the kernel build (`MODULE_SIG_ALL` off);
  `bin/steep-fetch-gpu` signs the NVIDIA modules from `MODULE_SIG_KEY` /
  `MODULE_SIG_KEY_PEM` and refuses a key that does not match the committed
  certificate. Generate the keypair before the next GPU image build — see
  `docs/module-signing.md`

### Added
- `confos build --profile-dir <dir>`: enable an mkosi profile from an
  out-of-tree directory (copied under `mkosi.profiles/<basename>` for the
  build's duration, enabled like `--profile <basename>`), so a consumer
  repo can own its image profile while this repo stays the builder. The
  directory must be self-contained — symlinks and `Include=` paths in its
  config tree pointing outside it are rejected, since staging re-parents
  them (links under its own `mkosi.extra/` are exempt, being image-relative)
  ([c8s#264](https://github.com/confidential-dot-ai/c8s/issues/264))
- `confos build --sync-input NAME=VALUE`: stage a value as
  `mkosi.local/.confos-sync-inputs/<NAME>` for profile sync hooks — the
  sanctioned tunnel for consumer inputs (sudo strips the environment mkosi
  runs under), replacing hand-written files in this repo's tree

### Changed
- `confos build` takes an exclusive whole-checkout lock (`.confos-build.lock`)
  for the duration of the run. Builds already could not overlap — they share
  `mkosi.local/`, `mkosi.profiles/`, `mkosi.output/` and `output/` — but the
  second one raced silently instead of saying so; it now fails immediately.
  The kernel releases the lock on process death, hard kills included, which
  is what lets the recovery below tell "abandoned" from "in use" without
  guessing

### Fixed
- A hard-killed build no longer leaks staged inputs into the next one. Both
  staging paths run cleanup on the way *in* as well as out: the
  `--sync-input` directory is cleared (so a later build that passes none of
  its own can't silently inherit, say, the wrong component ref), and orphaned
  `--profile-dir` copies are swept from `mkosi.profiles/` on every build —
  previously one could be picked up by a plain `--profile <name>`, or
  committed into this repo by `git add -A`

## [0.3.0] — 2026-08-07

### Added
- GPU confos profile (#68)
- `bin/lint` gates apt mirrors: every mirror must be a time-pinned
  `snapshot.ubuntu.com` URL, and every package-installing image (and
  configured tools tree) must declare one — an outage workaround can no
  longer outlive the outage (#79)

### Changed
- **Changes measurements.** Apt mirrors re-pinned to `snapshot.ubuntu.com`
  (base + tools `20260430T000000Z`, kernel-builder `20260405T000000Z`),
  removing the TEMP(2026-07-06) `archive.ubuntu.com` outage workaround, and
  the verity initrd — previously never mirror-pinned — now builds from the
  base snapshot. The next build of an otherwise-unchanged config rolls its
  measurement once, after which rebuilds stop drifting with build date (#79)
- c8s profile: default `C8S_REF` is the pinned published c8s commit
  `3a2517b` (the deployed release) instead of tracking `main`; CI still
  overrides per build (#79)

### Fixed
- GPU: udev probe held until FLR completes (#76); slow or degraded GPU
  bring-up no longer fails boot (#77)
- attestation-api: per-GPU nvidia char devices allowed (#73)
- c8s profile: node-image admission inventory and a reproducible GPU
  module build (#74); nested cluster networking kept reachable (#75)
- Boot: 120s wait-online stall from the phantom `dummy0` link (#78)
- Build manifests record the `image.raw` checksum (#55)

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

[Unreleased]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/confidential-dot-ai/confidential-os-builder/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/confidential-dot-ai/confidential-os-builder/releases/tag/v0.1.0
