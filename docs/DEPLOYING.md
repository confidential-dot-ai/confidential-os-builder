# Deploying Steep Images

`steep run` is a development harness. This guide covers what changes when
you take the artifacts to production hardware: host requirements, moving
images around, attaching disks without `steep run`, and operational policy
around measurements.

## Host requirements

Building and running have different requirements. Any Ubuntu machine that
can run mkosi builds images; only the *deployment* host needs TEE hardware.

### AMD SEV-SNP

- **CPU/firmware**: EPYC Milan or later with SNP enabled in BIOS (SME/SNP
  options), and host SEV firmware recent enough for your kernel's KVM.
- **Host kernel**: Linux 6.11 or later (first release with KVM SEV-SNP guest
  support), with `kvm_amd` loaded with `sev_snp=1` (check
  `/sys/module/kvm_amd/parameters/sev_snp`). `/dev/sev` must exist.
- **QEMU**: a build with SNP *and* IGVM support — the `sev-snp-guest` object
  and the `igvm-cfg` property (requires QEMU built against `libigvm`).
  `steep run` probes for exactly these two features when deciding whether
  the SNP tier is available; you can use the same probe:
  `qemu-system-x86_64 -object help | grep sev-snp` and
  `qemu-system-x86_64 -object sev-snp-guest,help | grep igvm`.
- **Guest artifacts**: the `guest-smp<N>.igvm` matching your vCPU count.
  The firmware is inside the IGVM — do not pass `-bios`/`-drive if=pflash`.

The essential QEMU shape (this is what `steep run` generates; add your own
networking and management options):

```bash
qemu-system-x86_64 \
  -machine q35,confidential-guest-support=sev0 \
  -object sev-snp-guest,id=sev0,cbitpos=51,reduced-phys-bits=1,igvm-file=guest-smp4.igvm \
  -smp 4 -m 8G \
  -drive file=disk.raw,format=raw,if=virtio \
  -nographic
```

**vCPU count must match an IGVM variant** — SMP is part of the launch
measurement. Memory size is not; size it freely. If you need a vCPU count
the build didn't emit, generate it without rebuilding:

```bash
steep igvm output/myimage --smp 12 --firmware output/OVMF.fd
```

### Intel TDX

- **CPU/firmware**: 4th-gen Xeon Scalable (Sapphire Rapids) or later with
  TDX enabled in BIOS, plus a host kernel and QEMU with TDX support (host
  enablement has been merging into mainline over recent kernels; distro
  TDX-enabled stacks — e.g. Ubuntu's intel-tdx builds — are the practical
  path today).
- **Guest artifacts**: `uki.efi` + `disk.raw`, booted with the TDVF-capable
  firmware recorded in the manifest (`OVMF.tdx.fd` in the output dir — its
  hash *is* the manifest's `mrtd`, so using any other firmware binary fails
  attestation). The IGVM files are not used on TDX.
- Memory and vCPU count are both free to vary — the manifest's TDX block is
  topology-invariant by design.

## Moving artifacts

`steep push` / `steep pull` transfer the whole output directory (manifest
included) through any OCI registry via `oras`:

```bash
steep push output/web --tag v1                    # defaults to ghcr.io/confidential-dot-ai/steep
steep push output/web --registry ghcr.io/you/imgs --tag v1
steep pull web --registry ghcr.io/you/imgs --tag v1
```

Verify what you pulled before booting it — hashes in the manifest, then the
manifest against your trusted copy ([VERIFYING.md](VERIFYING.md) §1).

### KubeVirt

`steep push --cdi` packs the artifacts into the single `tar+gzip` layer
layout (with `disk.raw` under `disk/`) that KubeVirt CDI's registry importer
expects, so a `DataVolume` can import the disk straight from the registry:

```yaml
source:
  registry:
    url: "docker://ghcr.io/you/imgs/web:v1"
```

Steep's guest kernel is built for KubeVirt SNP quirks (KubeVirt forces
`iommu_platform=true` on virtio devices; the required kernel config handles
this). You still need a KubeVirt version and node stack with SEV-SNP + IGVM
support to get measured launches; without it the image boots as a plain VM
with no attestation.

## Storage

### Ephemeral scratch (expanded writable space)

Outside `steep run --scratch`, attach any block device as a virtio disk
whose **virtio-block serial is `confai-scratch`** — the initrd matches on
the serial, not a filesystem label, so the device needs no formatting or
partitioning at all:

```bash
-drive file=/dev/nvme1n1,format=raw,if=none,id=scratch0,cache=none \
-device virtio-blk-pci,drive=scratch0,serial=confai-scratch
```

At boot the initrd generates a random key in RAM, opens the device as
dm-crypt (aes-xts-plain64, 512-bit key), formats it ext4, and mounts it as
the root overlay's upper layer. The host sees only ciphertext; the key is
never persisted, so contents are unrecoverable after the guest stops. Size
it for your workload's runtime writes — without it, writes land in a 2G RAM
tmpfs.

### Persistent data

Steep currently has **no persistent-disk convention**: everything under `/`
is either measured-and-read-only (the verity root) or ephemeral (the
overlay). If your workload needs durable state, attach an additional disk
and manage it from the workload itself — and remember the host reads and
tampers with attached storage freely, so the workload must bring its own
encryption *and* integrity protection (e.g. dm-crypt + dm-integrity keyed
from a secret released only after attestation). Treat host-visible
plaintext storage as published.

## Operational policy

- **Publish measurements out-of-band.** The manifest travels with the image
  for convenience, but verifiers must get their reference copy from a
  channel the image-hosting registry can't tamper with (your repo, a signed
  release). Consider signing manifests (e.g. cosign) as part of your release
  process.
- **Secrets only after attestation.** Images are world-readable
  ([THREAT_MODEL.md](THREAT_MODEL.md)); the deployment pattern is: boot →
  guest attests to a key broker / secret service → verifier checks the
  launch digest against the current allowlist → secrets released into
  encrypted guest memory.
- **Retire measurements like credentials.** When you ship a fix (kernel CVE,
  workload update), remove the old build's digests from your verifier's
  allowlist — an attacker can keep launching the old, correctly-measured
  image forever.
- **Keep the manifest with the fleet config.** `steep run` reads `memory`
  and picks IGVM variants from it; your orchestration should similarly
  treat the manifest as the source of truth for how the image expects to be
  launched.
