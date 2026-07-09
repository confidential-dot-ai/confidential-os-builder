# manifest.json Reference

`manifest.json` is written into every build's output directory and is the
authoritative description of what the build contains: input hashes, output
hashes, and the reference measurements a verifier compares attestation
reports against. This documents schema **version 3**, the current version
(`src/manifest.rs` is the source of truth).

Steep rejects manifests whose `version` differs from the version it was
built with — there is no cross-version compatibility shim. A v2 manifest
fails with an explicit "rebuild with the current steep" error rather than a
field-by-field parse error. Unknown fields are also rejected
(`deny_unknown_fields`), so a manifest that parses is exactly the documented
shape.

## Example

```json
{
  "version": 3,
  "build": {
    "timestamp": "2026-07-09T12:34:56Z",
    "memory": "4G",
    "format": "disk",
    "platform": "both"
  },
  "inputs": {
    "kernel": {
      "linux_version": "6.12.94",
      "vmlinuz_sha256": "…",
      "required_config_sha256": "…",
      "hardening_config_sha256": "…",
      "kernel_extra_config_sha256": "",
      "snapshot_config_sha256": "…"
    },
    "initrd": { "path": "initrd.cpio.zst", "sha256": "…" },
    "firmware": { "path": "OVMF.fd", "sha256": "…" },
    "base_image": { "path": "image.raw", "sha256": "…" }
  },
  "outputs": {
    "disk_image": { "path": "disk.raw", "sha256": "…" },
    "uki": { "path": "uki.efi", "sha256": "…" }
  },
  "snp_variants": [
    {
      "smp": 4,
      "igvm": { "path": "guest-smp4.igvm", "sha256": "…" },
      "measurement": {
        "snp_launch_digest": "…96 hex chars…",
        "algorithm": "sha384",
        "page_count": 5598,
        "vmsa_count": 4
      }
    }
  ],
  "tdx": {
    "mrtd": "…96 hex chars…",
    "rtmr1": "…96 hex chars…",
    "rtmr2": "…96 hex chars…",
    "firmware": { "path": "OVMF.tdx.fd", "sha256": "…" }
  }
}
```

## File entries

Fields typed as *file entry* are `{ "path": "...", "sha256": "..." }`.
`path` is a **basename only** (no directories), so manifests are portable
across hosts — resolve it relative to the directory containing the manifest.
`sha256` is the lowercase-hex SHA-256 of the file contents.

## Fields

### `version` (integer)

Schema version. Currently `3`. v3 introduced per-SMP SNP variants in
`snp_variants[]` and the singleton `tdx` block.

### `build` — how the build was invoked

| Field | Meaning | Load-bearing? |
|---|---|---|
| `timestamp` | Wall-clock time of the build (UTC). The **only** field expected to differ between two reproducible builds of the same inputs — exclude it when diffing manifests. | No — informational |
| `memory` | The `--memory` value. Read by `steep run` to size the VM; not part of any measurement (on TDX this is exactly what the RTMR[0]-unpinning design absorbs). | No — runtime default |
| `format` | Output image format (`disk`). | No |
| `platform` | Which measurement passes ran: `snp`, `tdx`, or `both`. Tells a verifier which measurement blocks to expect. | Indirectly |

### `inputs` — what went into the build

These identify the exact inputs, primarily so reproduction attempts can
bisect a mismatch to the diverging input. They are *descriptive*, not
independently verifiable by a third party (you generally don't have the
intermediate files).

| Field | Meaning |
|---|---|
| `kernel.linux_version` | Upstream kernel version compiled (from `kernel/version`) |
| `kernel.vmlinuz_sha256` | Hash of the compiled kernel binary embedded in the UKI |
| `kernel.required_config_sha256` / `hardening_config_sha256` | Hashes of steep's two always-applied config fragments |
| `kernel.kernel_extra_config_sha256` | Hash of the caller's `--kernel-config-fragment`; empty string when none was passed |
| `kernel.snapshot_config_sha256` | Hash of the fully-resolved `.config` lockfile (`kernel/config-x86_64.snapshot`) |
| `initrd` | The mkosi-built initrd **including** the prepended trusted-DSDT early cpio — i.e. exactly the initrd bytes inside the UKI |
| `firmware` | The SNP-side, IGVM-aware OVMF (`--firmware`). Absent for `--platform tdx` builds. Note this is *not* the TDX firmware — that lives at `tdx.firmware` |
| `base_image` | The mkosi-produced base filesystem image before disk assembly |

### `outputs` — what the build produced

| Field | Meaning | Load-bearing? |
|---|---|---|
| `disk_image` | `disk.raw` — GPT disk (ESP + erofs root + verity hash partitions) | **Yes** — verify on receipt |
| `uki` | `uki.efi` — the Unified Kernel Image (kernel + initrd + cmdline incl. verity roothash) | **Yes** — verify on receipt; also an input to both platforms' measurements |

### `snp_variants[]` — AMD SEV-SNP reference measurements

One entry per `--smp` value, because vCPU count is part of the SNP launch
measurement (each vCPU contributes a measured VMSA page). Populated by
`steep build`; `steep igvm` can extend or regenerate entries in place.
Omitted entirely for `--platform tdx` builds.

| Field | Meaning |
|---|---|
| `smp` | vCPU count this variant is measured for. **Match this to the deployed guest's vCPU count** |
| `igvm` | The IGVM file for this variant |
| `measurement.snp_launch_digest` | **The** SNP reference value: SHA-384 (96 hex chars) that must equal the `MEASUREMENT` field of a hardware attestation report from this guest |
| `measurement.algorithm` | `sha384` |
| `measurement.page_count` | Number of pages measured into the digest (diagnostic — useful when a digest mismatch needs bisecting) |
| `measurement.vmsa_count` | Number of VMSA pages measured; equals `smp` |

### `tdx` — Intel TDX reference measurements

A single block, valid for **any** memory size and vCPU count — the
trusted-DSDT override removes the only topology-sensitive content from the
measured surface. Omitted for `--platform snp` builds. All register values
are SHA-384, 96 lowercase hex chars.

| Field | Meaning |
|---|---|
| `mrtd` | Expected MRTD: hash of the TDVF firmware's measured regions, computed by simulating the TDX module's `MEM.PAGE.ADD` + `MR.EXTEND` sequence |
| `rtmr1` | Expected RTMR[1]: UKI PE image Authenticode hash + GPT event + boot-service constants |
| `rtmr2` | Expected RTMR[2]: UKI section measurement chain (.linux, .osrel, .cmdline, .initrd, systemd-stub Event 14) |
| `firmware` | The TDVF-capable OVMF binary (`--tdx-firmware`) these values were computed against — distinct from `inputs.firmware`. May be absent in manifests from before the dual-firmware split |

`rtmr0` is deliberately absent — see [THREAT_MODEL.md](THREAT_MODEL.md) and
the README's "Trusted DSDT" section for why it is left unpinned and what
compensates.

## Stability policy

- The schema evolves with steep; a `version` bump means older manifests must
  be regenerated by rebuilding (guaranteeing measurements are recomputed,
  never migrated by translation).
- Within a version, fields marked `Option`/defaultable above may be absent
  in manifests written by older point releases; consumers should treat
  absent-vs-empty per the table notes.
- Field semantics never change silently within a version. If you build
  tooling against the manifest, pin the `version` you support and fail
  loudly on others — exactly what steep itself does.
