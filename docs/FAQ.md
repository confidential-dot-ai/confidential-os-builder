# FAQ

### How is steep different from using mkosi directly?

mkosi builds general-purpose OS images; steep *uses* mkosi for the rootfs
and adds the confidential-computing layer on top: a pinned hardened guest
kernel, dm-verity + UKI assembly wired for measured boot, bit-identical
reproducibility of the base image, IGVM generation, offline pre-computation
of SNP launch digests and TDX registers, the trusted-DSDT mitigation, and a
manifest that ties inputs to expected measurements. If you don't need
attestation, use mkosi directly — steep's guest-oriented choices (read-only
verity root, ephemeral writes, no console) would just get in your way.

### How does steep compare to Confidential Kubernetes?

[c8s](https://confidential.ai/docs/c8s) is a full
confidential Kubernetes distribution — nodes, attestation service, cluster
lifecycle. Steep sits a level below: it builds and measures single VM
images and hands you the artifacts plus reference values; what orchestrates
them (QEMU scripts, libvirt, KubeVirt, your own control plane) is your
choice. Steep publishes reference measurements, and those can be verified by anyone executing workloads inside Confidential VMs running the Steep images.

### Why QEMU/KVM only for SEV-SNP?

The SNP launch digest depends on *exactly* how the VMM orders and flushes
IGVM directives and initializes VMSAs. `igvm-tools` replicates QEMU+KVM's
behavior byte-for-byte; a different VMM (cloud-hypervisor, Hyper-V) would
compute a different digest from the same IGVM. Support for other VMMs is on
the igvm-tools roadmap.

### Does steep support Intel TDX or only AMD?

Both. One build (`--platform both`, the default) emits artifacts and
reference measurements for either fleet: `snp_variants[]` launch digests for
SEV-SNP and an `mrtd`/`rtmr1`/`rtmr2` block for TDX. The same `uki.efi` and
`disk.raw` boot on both; only the firmware differs (steep's IGVM-aware OVMF
for SNP, a TDVF-capable OVMF for TDX).

## Security model

### Can the host read my data?

Three different answers:

- **Guest RAM**: no — encrypted by the hardware with a key the host lacks.
- **The image (`disk.raw`)**: yes, entirely. It's integrity-protected
  (dm-verity), not encrypted. Never bake secrets into an image; release
  secrets to the guest only after attestation.
- **Scratch disk**: no — encrypted in-guest with a boot-time random key the
  host never sees.

See [THREAT_MODEL.md](THREAT_MODEL.md) for the full picture.

### Why is RTMR[0] not in the manifest?

RTMR[0] captures VMM-supplied boot data (TD-HOB, ACPI tables) that varies
with memory size and vCPU count — pinning it would need a manifest entry per
topology. The dangerous part of that VMM-supplied data is the DSDT's
executable AML, and steep neutralizes that specifically: the measured initrd
overrides the VMM's DSDT with a trusted copy. So the manifest pins MRTD +
RTMR[1] + RTMR[2], which transitively cover everything that executes.

### Why does steep fork edk2?

Two reasons, both SNP+IGVM-specific: OVMF needs an `IgvmHobArea` region for
steep to inject the UKI into the measured launch payload, and it must skip
re-validating pages the PSP already validated during IGVM launch (stock
OVMF fails with page-not-validated errors). TDX uses stock OVMF/TDVF — the
fork deliberately does not include TDVF, which is why a both-platform build
uses two firmware binaries.

### Is Secure Boot involved?

The trust chain here is measurement-based, not signature-based: the
hardware attests a hash of what booted, instead of firmware enforcing
signatures at each stage. `igvm-tools` has flags for embedding Secure Boot
certificates and a shim for setups that want both, but steep's standard
pipeline relies on the launch measurement plus dm-verity.

## Practicalities

### Do I need SEV-SNP or TDX hardware to build images?

No. Building and computing measurements are offline operations; any Ubuntu
machine meeting the README's host requirements works, and CI does it on
stock GitHub runners. You need TEE hardware only to *run* guests with real
attestation — `steep run` falls back to plain KVM or emulation elsewhere.

### Why do two builds of the same config produce identical hashes — and when don't they?

Steep pins its inputs (kernel tarball by SHA, Ubuntu package snapshot,
toolchain via mkosi) and scrubs nondeterminism (timestamps, ordering), so
the base image is bit-identical across consecutive builds with the same
pinned toolchain. Reproducing on a *different* machine additionally
requires the same steep commit and the `bin/setup`-installed tool versions,
and hasn't been validated as broadly; see
[REPRODUCIBILITY.md](REPRODUCIBILITY.md) for exactly what's still open.

### My image needs more kernel features / more packages / my app. Where does each go?

- Userspace files or binaries → `--extra ./dir` (copied onto the rootfs).
- Ubuntu packages → `--package name`.
- Boot-time configuration → `--cloud-init user-data.yaml`.
- Build-time setup with network → `--script setup.sh`.
- Kernel options → a fragment file passed with `--kernel-config-fragment`
  (steep carries no project-specific kernel config; your fragment lives in
  your repo).

All are measured — each changes the image's digests, which is the point.

### Why is there no SSH / console / shell in my image?

By design: the attack surface is what you bake in, nothing more. For
interactive debugging build a separate image with `--profile dev` (serial
root autologin) — its measurement differs from production's, so it can't be
confused for it. For production debugging, your workload has to bring its
own (attested, authenticated) channel.

### Can I run containers inside a steep guest?

Yes, in principle — it's a normal Linux with systemd — but container
runtimes need kernel features steep's minimal baseline omits and disk space
beyond the 2G tmpfs overlay. Expect to supply a kernel fragment (netfilter,
overlayfs-in-userns, cgroup options, …) and a scratch disk. Note that
anything pulled at runtime is *not* measured; only what is included in the
built image is verified as part of an attestation.
