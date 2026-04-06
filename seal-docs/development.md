# Development

## Reproducibility

### Why it matters

Remote attestation requires a verifier to compare a hardware-signed measurement against an expected value. If the same inputs produce different measurements on each build, the verifier has nothing stable to compare against. Before this work, two consecutive `steep seal` runs with identical config produced completely different roothashes.

### What's reproducible

Boot-time cloud-init images (the production target) are bit-identical across builds. The same mkosi config + cloud-init YAML produces the same roothash, UKI hash, and IGVM measurement every time.

Bake-mode images are not reproducible and are not intended to be. They are verified via artifact signing instead of reproduction.

### Sources of non-determinism we fixed

| Source | Layer | Fix |
|--------|-------|-----|
| ext4 Directory Hash Seed | Filesystem | `SYSTEMD_REPART_MKFS_OPTIONS_EXT4=-E hash_seed=<fixed-uuid>` in mkosi.env |
| Partition UUIDs | Filesystem | `Seed=<fixed-uuid>` in mkosi.conf |
| Verity salt | Filesystem | `Seed=` derived via systemd PR #28695 (systemd 255+) |
| File mtimes | Files | `SourceDateEpoch=0` in mkosi.conf |
| /etc/machine-id | Files | Truncated in mkosi.finalize |
| /var/lib/dbus/machine-id | Files | Truncated in mkosi.finalize (conditional, file may not exist) |
| dpkg/apt/alternatives logs | Files | Deleted in mkosi.finalize |
| apt cache, ldconfig aux-cache | Files | Deleted in mkosi.finalize |
| random-seed files | Files | Deleted in mkosi.finalize |
| Cargo global cache | Files | Deleted in mkosi.finalize |
| Incremental=true build cache | Build | Set to false |

### Key insights

**ext4 hash_seed is the critical variable.** Even with `SOURCE_DATE_EPOCH=0` and a fixed `Seed=`, ext4 filesystems differ because `mkfs.ext4` generates a random Directory Hash Seed on each invocation. This seed is used for HTree directory indexing and changes the on-disk layout. The fix passes `-E hash_seed=<fixed-uuid>` to mkfs.ext4 via the `SYSTEMD_REPART_MKFS_OPTIONS_EXT4` environment variable. This variable is undocumented for ext4 in `repart.d(5)` but the `mkfs_options_from_env` function constructs the variable name dynamically from the filesystem type. Confirmed working on systemd 257.

**mkosi Environment= splits on spaces.** mkosi's `Environment=` config option splits values on spaces ([systemd/mkosi#2962](https://github.com/systemd/mkosi/issues/2962)). This breaks `-E hash_seed=...` because the flag and value become separate tokens. The fix uses `EnvironmentFiles=mkosi.env` which reads KEY=VALUE format without splitting.

**Finalize script ordering matters.** The reproducibility cleanup (truncate machine-id, delete logs/caches) must run after the bake block in `mkosi.finalize`. The bake block runs `apt-get update`, `cloud-init init`, etc., all of which recreate the files being cleaned.

### Apt snapshot pinning

Without pinning, the live Ubuntu mirror can change between builds. A security patch to any installed package changes the image content, cascading all the way up to the IGVM measurement.

The apt mirror is pinned to Ubuntu's snapshot service:

```ini
[Distribution]
Mirror=https://snapshot.ubuntu.com/ubuntu/20260405T000000Z
```

This freezes package state to a point-in-time. Snapshots are retained for at least 2 years.

To update: bump the snapshot timestamp in mkosi.conf, rebuild, record the new measurement, commit. Updates are always intentional and tracked in git.

| Event | Roothash | UKI | IGVM measurement |
|-------|----------|-----|------------------|
| Userspace package update | Changes | Changes | Changes |
| Kernel update | Changes | Changes | Changes |
| Cloud-init config change | Changes | Changes | Changes |
| No changes, same mirror state | Identical | Identical | Identical |

### Stress test results

| Test | Packages | Cloud-init | Roothash Repro | UKI Repro |
|------|----------|------------|----------------|-----------|
| Bare image | stock | none | Yes | Yes |
| Extra packages (+nginx, jq, curl, python3, tree) | extended | none | Yes | Yes |
| write_files only | stock | write_files | Yes | Yes |
| packages directive | stock | packages | Yes | Yes |
| VRS (packages + write_files + runcmd) | stock | full | Yes | Yes |
| 4 different configs | stock | 4 variants | All unique | All unique |
| Pinned apt snapshot | stock | none | Yes | Yes |

Package additions are safe (finalize cleanup handles post-install artifacts). Cloud-init config is measured (different content = different roothash). The full pipeline is deterministic. Different configs produce distinct measurements with no collisions.

### Trust model

```
Verifier checks:
  1. SNP attestation report is hardware-signed (AMD VCEK)
  2. IGVM launch measurement matches published measurement
  3. Published measurement is signed by the operator (cosign/sigstore)
  4. (Optional) Verifier reproduces base image to confirm toolchain integrity
```

Base image reproducibility serves as an audit mechanism. The baked layer is verified via artifact signing. This follows the same model as Constellation (Edgeless Systems).

### How others solve this

| Project | Approach | Build system | Reproducible? |
|---------|----------|-------------|---------------|
| Constellation (Edgeless) | Baked image + dm-verity, Nix-pinned toolchain | mkosi + Nix | Base yes, full via Bazel+Nix |
| Flashbots BuilderNet | Full image repro, every operator produces identical TDX measurements | Yocto | Yes (strongest model) |
| CoCo / Kata (CNCF) | Boot stack measured, workloads verified via image signatures | Various | Boot stack only |
| AWS Nitro Enclaves | Baked EIF, PCRs computed at build time | Kaniko | Yes |
| Cloud Providers (Azure, GCP) | vTPM attestation, proprietary firmware | Internal | Firmware no |

---

## Design Decisions

### Boot-time cloud-init is the production path, not bake

Bake mode runs cloud-init in a chroot without systemd, PAM, user management, or SSH key generation. Half the cloud-init modules fail. Boot-time cloud-init runs in a fully booted system where everything works. The tradeoff is that boot-time attestation proves "this VM was told to do X" not "X happened." For most deployments, the boot-time guarantee is sufficient since the operator trusts their own config.

### Reproducibility targets boot-time images only

Bake mode involves live apt fetches, non-deterministic compilation, and cloud-init runtime state. Making this reproducible would require Nix-pinned toolchains, `CARGO_BUILD_JOBS=1`, `--remap-path-prefix`, and frozen apt snapshots inside the chroot. Significant complexity for a non-production path.

### Firewall rules via cloud-init, not steep code

The original `nftables.rs` injected `output policy drop`, silently breaking all outbound traffic including cloud-init's ability to reach the internet. Firewall policy is deployment-specific and doesn't belong in the image builder. Users declare rules in their cloud-init config.

### Bake failures are fatal, not warnings

A "successful" build that silently skipped user setup or SSH keys is worse than a failed build. The operator gets a measured, sealed image with missing content and no indication anything went wrong. If a module can't run in a chroot, use boot-time cloud-init instead.

### No chroot sandboxing for bake mode

The /dev bind-mount is not the real risk. runcmd execution as root is. The user-data is a trusted input authored by the operator. Proper sandboxing is significant effort for a non-production path. A CLI warning is emitted. Revisit if bake becomes production or untrusted user-data needs support.

### ext4 hash_seed via environment variable

ext4's Directory Hash Seed is the single biggest source of non-determinism in mkosi builds. The environment variable approach is undocumented for ext4 in `repart.d(5)` but confirmed working on systemd 257 by reading the source. Must use `EnvironmentFiles=` instead of `Environment=` due to mkosi's space-splitting bug.

### Autologin as debug-only option

In the SNP threat model the host controls the serial port. Autologin gives the host an authenticated root session. This was originally added during early development for convenience and should never have been in the base config. Now injected via `--debug` flag with RAII cleanup.

### modprobe instead of insmod

`insmod` requires exact paths and correct load order. `modprobe` handles dependency resolution automatically via `depmod`.

### switch_root instead of manual pivot

`switch_root` from util-linux is purpose-built for this operation. It atomically cleans up the initrd, moves mounts, and execs the new init. The manual approach was fragile.

### deny_unknown_fields on manifest structs

Prevents injection of additional fields into `manifest.json` that might be interpreted by future code. The manifest is a trust boundary between build and run phases.

### Apt snapshot pinning

Without pinning, the live Ubuntu mirror changes over time. A security patch to any package cascades to UKI and IGVM measurement. Updates are intentional and tracked in git.

### Raw disk format, not qcow2

dm-verity requires direct block access. Raw images are simpler, have no format-specific metadata that could vary between builds, and map directly to virtio block devices.

---

## Audit Findings

### Fixed

| ID | Finding | Fix |
|----|---------|-----|
| AUDIT-2 | Memory/format not validated before QEMU interpolation | Added validate_memory, format allowlist, manifest validation |
| AUDIT-6 | Manual chroot instead of switch_root | `exec switch_root /sysroot /sbin/init` |
| AUDIT-7 | CloudInitCleanup::drop off-by-one | Fixed to check dir.ends_with not parent.ends_with |
| AUDIT-8 | Roothash not validated in seal.rs | Length check (64/96/128) + lowercase normalization |
| AUDIT-9 | Roothash regex in init didn't normalize case | `tr '[:upper:]' '[:lower:]'` before validation |
| AUDIT-10 | Glob expansion in cmdline parsing | `set -f` before parse loop |
| AUDIT-11 | Bake cloud-init failures silently swallowed | All stages now fatal |
| AUDIT-12 | cloud-init clean --logs left semaphore files | Changed to cloud-init clean |
| AUDIT-13 | safe_path() was dead code | Removed |
| AUDIT-14 | base.rs used different mkosi resolution than seal.rs | Unified to resolve_mkosi() + sudo |
| AUDIT-15 | Autologin hardcoded in mkosi.conf | Removed, added --debug flag |
| AUDIT-16 | QEMU comma-injection in paths | reject_comma_in_path for all path args |

### Open

| ID | Finding | Status |
|----|---------|--------|
| AUDIT-5 | Launch digest not verified at run time | Documented as build-time only, runtime verification planned |
| AUDIT-18 | No tests for injection rejection | Tests planned |
| n/a | overflow-checks = true missing from release profile | Planned |
| n/a | Unreferenced util-linux-extra .deb in repo root | Planned removal |

---

## Changelog

### Reproducible builds
- Identified 8+ sources of non-determinism in mkosi builds and fixed all of them
- Discovered and worked around mkosi Environment= space-splitting bug using EnvironmentFiles=
- Created mkosi.finalize reproducibility cleanup with correct ordering (after bake)
- Added initrd reproducibility (SourceDateEpoch, Seed, finalize script)
- Pinned apt mirror to Ubuntu snapshot service

### Cloud-init
- Fixed DNS, apt source corruption, and empty apt lists in bake chroot
- Made all bake cloud-init stage failures fatal
- Changed cloud-init clean to full cleanup (was --logs only, left semaphore files)
- Removed nftables.rs (was silently breaking cloud-init networking)
- Fixed CloudInitCleanup::drop off-by-one

### Security hardening
- Added roothash validation (lowercase, 64/96/128 hex) in both init and seal.rs
- Added glob protection in cmdline parsing
- Replaced insmod with modprobe, manual pivot with switch_root
- Added nosuid,nodev to overlay tmpfs mount
- Added QEMU comma-injection rejection for all paths
- Added disk format allowlist and memory validation
- Added deny_unknown_fields on all manifest structs
- Removed autologin from base, added --debug flag with RAII cleanup

### Code quality
- Replaced .expect() calls with ? operator across qemu.rs and seal.rs
- Fixed bare QEMU binary name resolution via PATH
- Added sudo_copy and sudo_chmod_readable utilities
- Removed dead safe_path() function
- Unified base.rs mkosi resolution with seal.rs
- Added trap-based cleanup for chroot mounts (EXIT + INT + TERM)

### Tests
- Rewrote all 6 test files to match current API (33 tests passing)
- Complete rewrite of e2e.sh with lightweight cloud-init, IGVM seal test, reproducibility check, and manifest validation

### Verified end-to-end
- Seal, boot, dm-verity + overlayfs + cloud-init runcmd working on KVM
- Seal, IGVM, SNP launch, attestation report, digest exact match on AMD EPYC hardware
- Full orchestrator pipeline verified (orchestrator, SNP VM, kettle-server, cargo build, SLSA provenance, SNP attestation evidence)

---

## References

- [Reproducible Arch images with mkosi, Jelle van der Waa](https://vdwaa.nl/mkosi-reproducible-arch-images.html)
- [edgelesssys/reproducible-mkosi](https://github.com/edgelesssys/reproducible-mkosi)
- [Reproducible builds for confidential computing, Edgeless Systems](https://www.edgeless.systems/blog/reproducible-builds-for-confidential-computing)
- [systemd/systemd#28656, reproducible verity salt and UUID](https://github.com/systemd/systemd/issues/28656)
- [systemd/mkosi#2957, reproducible UKI](https://github.com/systemd/mkosi/issues/2957)
- [systemd/mkosi#1112, reproducible builds tracking](https://github.com/systemd/mkosi/issues/1112)
- [systemd/mkosi#2962, Environment= space-splitting bug](https://github.com/systemd/mkosi/issues/2962)
- [SOURCE_DATE_EPOCH specification](https://reproducible-builds.org/specs/source-date-epoch/)
- [FOSDEM 2024, Reproducible Builds for Confidential Computing](https://archive.fosdem.org/2024/schedule/event/fosdem-2024-1769-reproducible-builds-for-confidential-computing-why-remote-attestation-is-worthless-without-it/)
- [Constellation attestation architecture](https://docs.edgeless.systems/constellation/architecture/attestation)
- [CoCo attestation flow](https://confidentialcontainers.org/docs/attestation/)
- [IETF RFC 9334, RATS Architecture](https://datatracker.ietf.org/doc/rfc9334/)
