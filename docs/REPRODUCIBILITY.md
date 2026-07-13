# Reproducibility and Attestation Verification

## The Problem

Remote attestation requires a verifier to compare a hardware-signed measurement against an **expected value**. If the same inputs produce different measurements on each build, the verifier has nothing stable to compare against. We have implemented `steep build` so runs with identical config can produce completely identical roothashes, UKIs, and IGVM measurements.

## Our Approach

We achieve **bit-identical base images** from mkosi. The base image (Ubuntu + stock packages) produces the same roothash and UKI SHA256 across consecutive builds. This gives us a reproducible foundation, and is a common approach  for confidential computing deployments.

### Trust model

```
Verifier checks:
  1. SNP attestation report is hardware-signed (AMD VCEK) ✓
  2. IGVM launch measurement matches published measurement ✓
  3. Published measurement obtained via a trusted channel ✓
  4. (Optional) Verifier reproduces base image to confirm our toolchain is honest ✓
```

Steep does not yet sign published measurements (cosign/sigstore signing is
planned); until then step 3 rests entirely on the channel the verifier fetched
`manifest.json` from, e.g. this repository or a user's own build.

The base image reproducibility serves as an **audit mechanism** — anyone can
rebuild it to verify we aren't shipping a tampered base.

## Config for reproducibility

We use several techniques to ensure the results of our builds are reproducible, including `mkosi` configuration, environment variables, and package management configuration.

### mkosi.conf

| Setting | Value | Purpose |
|---------|-------|---------|
| `Incremental` | `false` | No stale build cache leaking between builds |
| `SourceDateEpoch` | `0` | Clamps all file mtimes to epoch 0 |
| `Seed` | `d4f09d27-7e4e-4b1a-9c3a-deadbeef0001` | Deterministic partition UUIDs, verity salt, GPT metadata |
| `EnvironmentFiles` | `mkosi.env` | Passes env vars to systemd-repart sandbox |

### mkosi.env

| Variable | Purpose |
|----------|---------|
| `SOURCE_DATE_EPOCH=0` | Propagated to dpkg/apt for timestamp clamping |
| `SYSTEMD_REPART_MKFS_OPTIONS_EXT4=-E hash_seed=<fixed>` | Deterministic ext4 directory hash seed (undocumented for ext4 but confirmed working on systemd 257) |

### apt mirror pinning

Identical package *versions* across builds are enforced by pinning
`Mirror=` (and `ToolsTreeMirror=`) in `mkosi/base/mkosi.conf` and
`mkosi/kernel-builder/mkosi.conf` to a point-in-time
`snapshot.ubuntu.com` URL. Bumping that timestamp is the deliberate act
that picks up security updates — and changes the roothash.

> **Current status:** as of 2026-07-06 both mirrors are temporarily
> reverted to the rolling `archive.ubuntu.com` mirror because of a
> snapshot-service outage (see the `TEMP` comment in each mkosi.conf).
> Until the pin is restored, package versions — and therefore
> measurements — can drift between builds.

mkosi itself is pinned to v26 — `bin/setup` and CI install exactly
`mkosi.git@v26`, and `mkosi.conf` enforces `MinimumVersion=26` as a
floor — since mkosi's own behavior is part of the build's determinism.
The Rust toolchain that builds the `steep` binary is *not* pinned:
steep's contributions to the measured artifacts (the DSDT early-cpio,
the IGVM file, the precomputed measurements) are deterministic data
derived from fixed inputs, so the compiler version doesn't affect
artifact bytes the way a package-set change would.

### mkosi.finalize

Reproducibility cleanup:
- Truncate `/etc/machine-id` and `/var/lib/dbus/machine-id`
- Delete dpkg/apt/alternatives logs, journal dir
- Delete apt cache, ldconfig aux-cache, man cache
- Delete random seeds, cargo cache
- Delete SSH host keys (regenerated per-VM on first boot)

## Open Questions

**Will adding packages to the base image break reproducibility?**
Adding packages to `Packages=` in mkosi.conf should remain reproducible as long as the same package versions are installed (same apt mirror state). The finalize cleanup handles the logs and caches that package installation creates. This needs testing.

## How Others Solve This

### Constellation (Edgeless Systems)
- Fully baked immutable image with dm-verity, architecturally closest to steep
- Base image built with mkosi, toolchain pinned via Nix
- Default: users fetch signed measurements from Edgeless's registry (cosign + Rekor transparency log)
- Paranoid path: reproduce from source via Bazel + Nix
- Per-deployment config (cluster identity) measured into a separate PCR, not the image
- Project has moved to Contrast (confidential containers)

### Flashbots BuilderNet
- Full image reproducibility required — every operator must produce identical TDX measurements
- Built with Yocto (exploring mkosi migration)
- Strongest trust model: decentralized, no single party trusted
- Reference measurements published at measurements.buildernet.org

### CoCo / Kata Containers (CNCF)
- TEE boot stack measured and requires reference values (small surface)
- Container workloads verified via image signatures (cosign/sigstore), not measurement matching
- Hardware attestation proves the policy enforcement engine is genuine
- Key Broker Service releases secrets only if attestation + signature policy passes

### AWS Nitro Enclaves
- Entire application baked into an immutable EIF
- PCR values computed at build time, explicitly support reproducible builds via Kaniko
- Trust root is AWS's Nitro Hypervisor (not CPU-level TEE)

### Cloud Providers (Azure, GCP)
- vTPM-mediated attestation with proprietary firmware
- Reference values managed internally (Azure MAA, Google Cloud Attestation)
- Firmware is closed-source and not reproducible — users must trust the provider
- Guest OS measurements are the user's responsibility

## References

- [Reproducible Arch images with mkosi — Jelle van der Waa](https://vdwaa.nl/mkosi-reproducible-arch-images.html)
- [edgelesssys/reproducible-mkosi](https://github.com/edgelesssys/reproducible-mkosi)
- [Reproducible builds for confidential computing — Edgeless Systems](https://www.edgeless.systems/blog/reproducible-builds-for-confidential-computing)
- [systemd/systemd#28695 — reproducible verity salt and UUID](https://github.com/systemd/systemd/pull/28695)
- [systemd/mkosi#2957 — reproducible UKI](https://github.com/systemd/mkosi/issues/2957)
- [systemd/mkosi#1112 — reproducible builds tracking](https://github.com/systemd/mkosi/issues/1112)
- [systemd/mkosi#2962 — Environment= space-splitting bug](https://github.com/systemd/mkosi/issues/2962)
- [FOSDEM 2024 — Reproducible Builds for Confidential Computing](https://archive.fosdem.org/2024/schedule/event/fosdem-2024-1769-reproducible-builds-for-confidential-computing-why-remote-attestation-is-worthless-without-it/)
- [Constellation attestation architecture](https://docs.edgeless.systems/constellation/architecture/attestation)
- [CoCo attestation flow](https://confidentialcontainers.org/docs/attestation/)
- [IETF RFC 9334 — RATS Architecture](https://datatracker.ietf.org/doc/rfc9334/)
- [SOURCE_DATE_EPOCH specification](https://reproducible-builds.org/specs/source-date-epoch/)
- [Flashbots BuilderNet v1.3](https://buildernet.org/blog/2025/04/28/buildernet-v1.3)
- [Trail of Bits — Notes on AWS Nitro Enclaves](https://blog.trailofbits.com/2024/02/16/a-few-notes-on-aws-nitro-enclaves-images-and-attestation/)
- [Confidential Computing Transparency Framework](https://arxiv.org/html/2409.03720v2)
