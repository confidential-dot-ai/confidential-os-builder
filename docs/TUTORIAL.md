# Tutorial: Zero to Attested Image

A guided first session with steep: build an image, boot it, put a real
workload in it, and find the measurements a verifier would check. The
[README](../README.md) is the reference for every flag; this is the
narrative version.

## 0. Prerequisites

A real Ubuntu Linux host (bare metal, a VM with nested virt, or a cloud
instance) with `sudo`. Rootless dev containers won't work — mkosi's sandbox
needs user-namespace capabilities they can't provide. You do **not** need
SEV-SNP or TDX hardware for anything in this tutorial; building and
measuring are entirely offline computations, and `steep run` falls back to
plain KVM or emulation for booting.

```bash
git clone https://github.com/confidential-dot-ai/steep.git
cd steep
bin/setup        # installs mkosi v26, qemu-utils, swtpm, iasl, ovmf, rust, cargo-nextest
sudo apt install qemu-system-x86   # the emulator itself — bin/setup does NOT install it
```

`bin/setup` installs everything `steep build` needs. The extra
`qemu-system-x86` package provides `qemu-system-x86_64`, which `steep run`
uses to boot images from step 2 onward.

## 1. Build the base image

```bash
bin/steep build
```

`bin/steep` compiles steep itself with cargo, then runs it. The first build
does a lot of one-time work — budget 20–40 minutes:

1. **Kernel** — downloads the pinned Linux source (`kernel/version`),
   resolves steep's hardened config, and compiles it (~10 min, cached
   afterwards in `output/kernel/`).
2. **Base image** — mkosi assembles a minimal Ubuntu root filesystem.
3. **Verity + UKI** — the rootfs becomes an erofs partition with a dm-verity
   hash tree; kernel + initrd + cmdline (containing the verity roothash)
   fuse into a single `uki.efi`.
4. **Measurements** — one IGVM + SNP launch digest per vCPU-count variant,
   plus the TDX register block, all recorded in the manifest.

When it finishes, look at what you got:

```bash
ls output/base/
# OVMF.fd  OVMF.tdx.fd  combined-initrd.img  disk.raw  dsdt.aml
# guest-smp2.igvm ...  manifest.json  roothash  uki.efi

jq '{platform: .build.platform,
     uki: .outputs.uki.sha256,
     snp_digests: [.snp_variants[] | {smp, digest: .measurement.snp_launch_digest}],
     tdx: {mrtd: .tdx.mrtd, rtmr1: .tdx.rtmr1, rtmr2: .tdx.rtmr2}}' \
   output/base/manifest.json
```

Those digest values are the whole point: a verifier compares a hardware
attestation report against them. See [MANIFEST.md](MANIFEST.md) for every
field and [VERIFYING.md](VERIFYING.md) for the comparison procedure.

## 2. Boot it — with a shell, for now

A production steep image is deliberately inhospitable: no console login, no
SSH, nothing but what you baked in. For poking around, build a **dev**
variant, which adds passwordless root autologin on the serial console and
`console=ttyS0` boot output:

```bash
bin/steep build devbox --profile dev --kernel-config-fragment kernel/dev.config
bin/steep run output/devbox
```

`steep run` picks the best available backend automatically — SEV-SNP if the
host supports it, plain KVM if not, software emulation as a last resort —
and drops you on the VM's serial console at a root prompt. Poke around:

```bash
findmnt /        # overlay on top of a dm-verity-protected erofs
dmesg | head -30 # the boot chain you just measured
poweroff         # exits QEMU, returns your terminal
```

Note that the dev image's measurements differ from the base image's — the
autologin drop-in lives in the measured rootfs and `console=ttyS0` is on the
measured cmdline. That's the design working: a dev image can never
impersonate a production one. Never deploy `--profile dev` (the host owns
the serial port).

## 3. Run a real workload

`examples/caddy.yaml` is a cloud-init file that installs the Caddy web
server and serves a page. Cloud-init user-data gets baked into the measured
rootfs — it is part of the image, not runtime configuration:

```bash
bin/steep build web --cloud-init examples/caddy.yaml
bin/steep run output/web --port-forward 8080:80
```

From another terminal:

```bash
curl http://localhost:8080/
```

That response came from inside a VM whose entire contents — Ubuntu, Caddy,
the HTML, the kernel that booted it — are captured by the digests in
`output/web/manifest.json`. Because user-data is measured, **never put
secrets in it**; the disk image is integrity-protected but not encrypted
(see [THREAT_MODEL.md](THREAT_MODEL.md)).

Beyond cloud-init, the other ways to get content into an image:

- `--extra ./dir` — files copied verbatim onto the rootfs (binaries,
  systemd units, static config).
- `--package curl,jq` — extra Ubuntu packages.
- `--script setup.sh` — a post-install script run during the image build,
  with network access.

All of them land in the verity root, so all of them are measured.

## 4. Give it disk space

The writable layer is a 2G RAM tmpfs by default. For workloads that need
room, attach an ephemeral encrypted scratch disk:

```bash
bin/steep run output/web --scratch 20G
```

The initrd encrypts it with a random key generated in-guest (held only in
RAM, never persisted), formats it, and uses it as the overlay's upper
layer — the whole filesystem transparently gains 20G. Contents are
ciphertext to the host and unrecoverable after shutdown.

## 5. Ship it

```bash
bin/steep push output/web                              # pushes ghcr.io/confidential-dot-ai/steep:web via oras
bin/steep pull ghcr.io/confidential-dot-ai/steep:web   # on another machine, pulls it into output/web
```

Publish `manifest.json` through a channel your verifiers trust — it carries
the expected measurements they'll check attestation reports against.

## Where to next

- [VERIFYING.md](VERIFYING.md) — attest a guest on real SNP/TDX hardware
- [DEPLOYING.md](DEPLOYING.md) — production hosts, KubeVirt, scratch disks
  outside `steep run`
- [THREAT_MODEL.md](THREAT_MODEL.md) — what all this does and doesn't protect
- [CONCEPTS.md](CONCEPTS.md) — ground-up explanations of UKI, dm-verity,
  IGVM, and the rest of the vocabulary
