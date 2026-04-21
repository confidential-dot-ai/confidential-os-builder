# Steep Seal Documentation



| Document | What it covers |
|----------|---------------|
| [Design](design.md) | Architecture, measurement chain, boot sequence, overlay model, threat model, security hardening |
| [Guide](guide.md) | Commands, cloud-init modes, QEMU launch, testing |
| [Development](development.md) | Reproducibility, design decisions, audit findings, changelog |

---

## Measurement Chain

The attestation model rests on a deterministic chain from source configuration to hardware-signed measurement.

```
cloud-init YAML
    |  injected into image as static file
    v
erofs root filesystem
    |  dm-verity hash tree
    v
roothash (SHA-256)
    |  embedded in kernel cmdline
    v
UKI (kernel + initrd + cmdline as one EFI binary)
    |  bundled with OVMF firmware
    v
IGVM (measured by SNP hardware on launch)
    |
    v
SNP launch digest (hardware-signed, unforgeable)
```

Change one file in the root filesystem and the roothash changes, which changes the UKI, which changes the IGVM measurement. A remote verifier checks the launch digest against a published expected value and can trust the entire stack.

## Output Artifacts

```
disk.raw         GPT disk image (ESP + root + verity partitions)
uki.efi          Unified Kernel Image
roothash         SHA-256 hex string of the root filesystem
manifest.json    Build metadata with hashes, platform, measurement
guest.igvm       IGVM file (optional)
```
