# Versioning and Measurement Stability

## Version scheme

Steep uses semantic versioning and is pre-1.0: minor releases (0.x → 0.y)
may break CLI flags, the manifest schema, and build behavior; patch releases
are fixes. The in-repo crates version independently (`igvm-tools`,
`tdx-measure`) since verifiers install them standalone.

The manifest schema carries its own version (`manifest.json`'s `version`
field, currently 3). Steep refuses to read manifests from a different
schema version — regeneration by rebuild is the only migration path, which
guarantees measurements are recomputed rather than translated. Tooling you
build on the manifest should do the same. See
[MANIFEST.md](MANIFEST.md#stability-policy).

## What invalidates published measurements

**When does upgrading something force you to re-publish expected measurements
and update verifier allowlists?**

A build's measurements change whenever any measured input changes:

| Change | Changes measurements? |
|---|---|
| Steep version bump | **Assume yes.** Even without a deliberate pipeline change, steep pins the kernel version, base distro snapshot, and image-assembly details; release notes in [CHANGELOG.md](../CHANGELOG.md) flag measurement-affecting changes explicitly |
| Kernel version or any config fragment (incl. your `--kernel-config-fragment`) | Yes |
| mkosi / toolchain version drift on the build host | Yes, potentially — this is why `bin/setup` pins mkosi and why reproduction requires the same tool versions |
| Your image content: cloud-init, `--extra`, `--package`, `--script` | Yes — by design |
| `--profile dev` on/off | Yes — by design |
| `--firmware` binary (SNP) | Yes — new IGVM launch digests |
| `--tdx-firmware` binary | Yes — new `mrtd` |
| `--smp` list | Adds/removes `snp_variants[]` entries; existing digests for unchanged counts are unaffected. TDX values unaffected |
| `--memory` | **No** — runtime default only, not measured on either platform |
| vCPU/memory shape at deployment (TDX) | No — the TDX block is topology-invariant |
| `steep push`/`pull`, registry, tags | No — transport only |

If you're using Steep to build your images:

- **Pin the steep commit** you build releases with, alongside your image
  config, so you can reproduce byte-identical artifacts later
  ([VERIFYING.md §3](VERIFYING.md#3-build-reproduction-audit)).
- **Treat a steep upgrade like an image change**: rebuild, re-publish the
  new manifest, add the new digests to verifier allowlists, and retire the
  old digests on your own schedule (old images remain launchable forever —
  see [THREAT_MODEL.md](THREAT_MODEL.md) on rollback).
- **Diff the kernel snapshot on upgrade** (`kernel/config-x86_64.snapshot`)
  — it shows exactly what changed in the resolved kernel config, which is
  usually the interesting part of a measurement change.
