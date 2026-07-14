# Threat Model

What confos-built images defend against, what they assume, and what they
explicitly do not protect. Read this before deploying confos images in
production; several guarantees have sharp edges (most importantly:
**the disk image is not confidential** — see below).

## Setting

A confos image is a guest VM designed to run on an **untrusted host**. The
operator of the physical machine — cloud provider, colo, or a compromised
hypervisor — is inside the attacker model. The hardware TEE (AMD SEV-SNP or
Intel TDX) is what removes the host from the trusted computing base:
guest memory is encrypted with a key the host never sees, and the CPU
measures what was loaded at launch and signs that measurement.

Confidential OS Builder's job is the *software* half of that story: producing
an image whose entire boot chain is deterministic, integrity-protected, and
captured by the hardware measurement, so that a remote verifier can know
precisely what code is running.

## Trusted

| Component | Why it must be trusted |
|---|---|
| CPU vendor silicon and firmware (AMD PSP / Intel TDX module) | Root of the measurement and memory-encryption guarantees. Vendor-signed; verified via the attestation certificate chain. |
| The confos toolchain and build inputs, at build time | Whatever you bake in is what runs. A compromised build host produces a correctly-measured malicious image. Mitigation: reproducible builds let third parties audit (see [REPRODUCIBILITY.md](REPRODUCIBILITY.md)). |
| The channel publishing expected measurements | Attestation compares against `manifest.json`. If the attacker controls your copy of the manifest, attestation proves nothing. |
| The workload itself | Confidential OS Builder attests *which* code launched, not that the code is bug-free. A vulnerable service inside the guest is still vulnerable. |
| Guest firmware (OVMF) | Measured into the launch digest (SNP) / MRTD (TDX), so a substituted firmware is *detected* — but the firmware you chose to measure is trusted to behave correctly. |

## Untrusted

| Component | Attack surface | Defense |
|---|---|---|
| Host / hypervisor / VMM | Reads or tampers with guest memory; substitutes boot artifacts; lies about devices | Hardware memory encryption; launch measurement covers firmware + UKI (kernel, initrd, cmdline); anything substituted changes the measurement |
| Attached storage (`disk.raw`, any block device) | Host can read and modify all disk content at rest and in flight | dm-verity: every root filesystem block is verified against a hash tree whose root is in the measured kernel cmdline. Tampering → I/O error, not silent corruption. **Integrity only — no confidentiality** (see below) |
| VMM-supplied ACPI tables (TDX) | The DSDT contains AML bytecode the guest kernel executes at kernel privilege ("BadAML") | The initrd carries a trusted, audited DSDT that overrides the VMM's at boot; the override mechanism is part of the measured initrd. RTMR[0] (which the VMM still influences) is deliberately left unpinned |
| Serial console | Host reads and injects console traffic | Production images have no console login. `--profile dev` adds a passwordless root autologin on ttyS0 — **never ship a dev-profile image**; the measurement changes, which is your detection mechanism |
| Network | Standard untrusted network | Out of confos's scope — the workload must use TLS etc. as usual |
| Virtio devices | Malicious device implementations probing guest drivers | Hardened kernel config trims the surface (no USB, no PCI hotplug, no DRM, lockdown LSM); virtio drivers themselves remain in the TCB |

## Guarantees

When a verifier follows [VERIFYING.md](VERIFYING.md) and the checks pass:

1. **Launch integrity** — the guest booted exactly the firmware, kernel,
   initrd, and kernel command line in the manifest. On SNP the IGVM launch
   digest covers all of it; on TDX it is covered by MRTD + RTMR[1] + RTMR[2].
2. **Root filesystem integrity** — every block of the root filesystem the
   guest ever reads matches the dm-verity root hash embedded in the measured
   cmdline. This transitively covers everything baked in at build time:
   packages, `--extra` files, cloud-init user-data, post-install script
   effects.
3. **Runtime memory confidentiality** — guest RAM is encrypted with a key
   the host does not have (hardware guarantee, not confos's).
4. **Scratch confidentiality** — the optional scratch disk is encrypted with
   a random key generated in-guest at boot and never persisted. The host
   sees only ciphertext; contents do not survive reboot.

## Explicitly not protected

- **Disk confidentiality.** `disk.raw` is a plaintext erofs filesystem plus
  verity hash tree. The host — and anyone you distribute the image to — can
  read every byte. **Never bake secrets into an image**: not in `--extra`
  files, not in cloud-init user-data, not via a post-install script.
  Provision secrets at runtime, released only after successful attestation
  (e.g. a key broker that verifies the launch measurement first).
- **Availability.** The host can pause, throttle, or kill the guest at any
  time. TEEs do not and cannot prevent denial of service.
- **Side channels.** Speculative-execution attacks, cache timing,
  memory-access-pattern and network-traffic analysis, and power/frequency
  analysis are out of scope. Confidential OS Builder's kernel hardening
  reduces some surface but makes no side-channel claims.
- **Runtime compromise.** If the workload has an exploitable bug, the
  attacker operates inside a perfectly-attested guest. Attestation is a
  statement about launch state, not ongoing behavior.
- **Attestation freshness / rollback.** A launch measurement does not expire.
  Verifiers must supply a fresh nonce per attestation (see
  [VERIFYING.md](VERIFYING.md)) and decide their own policy for retiring old
  measurements (e.g. after a kernel CVE, stop accepting digests of images
  built with the vulnerable kernel).
- **Host-controlled time and entropy at boot.** The guest's initial clock
  comes from the host. Confidential OS Builder's kernel trusts the CPU's
  RDRAND/RDSEED for early entropy (standard for CVMs); if your threat model
  excludes trusting the CPU vendor here, it already excludes the TEE itself.
- **TCB vulnerabilities.** Flaws in AMD PSP / SEV firmware, the Intel TDX
  module, or the measured guest kernel undermine the guarantees. Track
  vendor TCB recovery events; SNP attestation reports include TCB version
  info a verifier should check against current baselines.

## Design decisions with security implications

- **RTMR[0] unpinned on TDX** — deliberate; the trusted-DSDT override closes
  the executable-AML gap, and what remains in RTMR[0] (TD-HOB, non-DSDT
  ACPI data tables varying with topology) is data the hardened kernel treats
  as untrusted input. This is a tradeoff, allowing different amounts of
  memory and different numbers of CPU cores while measuring everything else.
- **Writable state is an overlay, ephemeral by default** — with no scratch
  disk, writes go to a RAM tmpfs and vanish on shutdown.
- **`--profile dev` changes the measurement** — this is the feature. A dev
  image can never silently pass verification as a production image.

## Reporting

Anything that breaks these guarantees in practice — unmeasured content
reachable in the verity root, measurement computation errors,
non-reproducible builds that should reproduce, `confos run` weakening the
documented posture — is a security bug. See [SECURITY.md](../SECURITY.md)
for private reporting.
