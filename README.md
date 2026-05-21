# 🍵 steep, secure VM image builder

Steep is a confidential VM image builder for AMD SEV-SNP. It produces
bit-identical, attestable disk images from declarative configuration. The `build`
command builds a dm-verity protected root filesystem, bundles it into a Unified
Kernel Image (UKI), and optionally wraps it in an IGVM for measured launch on
SNP hardware.

| Document | What it covers |
|----------|---------------|
| [Concepts](docs/CONCEPTS.md) | Architecture, boot steps, image structure to ensure security |
| [Reproducibility](docs/REPRODUCIBILITY.md) | Changes needed, prior art, comparisons to other work |

## Running steep-built VMs

You can use the base images built by `steep` without installing the tool.

```
mkdir steep-base; cd steep-base
oras pull ghcr.io/lunal-dev/steep/base:latest
qemu-system-x86_64 \
  -machine q35 \
  -kernel uki.efi \
  -drive if=pflash,format=raw,readonly=on,file=OVMF.fd \
  -drive file=disk.raw,format=raw,if=virtio \
  -smp 1 -m 4G -nographic
```

## Installation

Steep runs on Ubuntu Linux. Clone the steep repo and run `bin/setup` to install everything you'll need (mkosi v26, qemu utils, swtpm, rust, cargo-nextest).

```bash
git clone https://github.com/confidential-ai/steep.git
cd steep
bin/setup
cargo build --release
```

The build host needs to be a real Linux system with `sudo` and the kernel/userns capabilities to run mkosi's sandbox. Most rootless dev containers can't — their nested user namespace can't `chown` to arbitrary uids/gids during package extraction, which mkosi requires.

## Scope

Steep builds **SEV-SNP guest images** — measurable, dm-verity-protected, attestable VM images that boot inside an L0 hypervisor. It is **not** a builder for host/hypervisor images that themselves run other VMs: steep's guest-oriented kernel, IGVM measurement, and verity initrd are all designed for the guest side of the trust boundary. For a plain host or hypervisor image, use a general-purpose image builder such as mkosi directly.

## Usage

### 1. Build a base VM image

```bash
steep build
```

Steep uses `mkosi` to build base image for Ubuntu 26.04 (Resolute Raccoon).
Built images is fully measured (see [Measurement Chain](#measurement-chain)),
and suitable for booting trusted confidential VMs.

Pass `--console` to enable a passwordless root autologin on the serial console,
so `steep run` pops a shell. This changes the image measurement and must not be
used for production images — under the SNP threat model the host controls the
serial port.

### 2. Build a VM image

```bash
steep build NAME -c path/to/cloud-init/user-data
```

This will build and measure a new image, including the cloud-init file. The results will be written to `output/NAME`,
ready to be run with `steep run output/NAME` or pushed to GHCR with `steep push output/NAME`.

Pass `--script PATH` (or `-s PATH`) to run a custom post-install script
during the build. The script is forwarded to mkosi as a `--postinst-script`
with `--with-network=yes`, so it can download resources from the network.
Inside the script, `$BUILDROOT` points at the image filesystem being
assembled.

### 3. Launch the built VM

```bash
steep run output/dir
```

Use qemu to boot the VM image with a software-emulated TPM and the cidata ISO
attached. After boot, the VM will run cloud-init, if a config file was included
in the build. To get an interactive shell on boot, build the image with
`--console` (see above).

## Measurement Chain

The attestation model rests on a deterministic chain from source configuration to hardware-signed measurement.

```
cloud-init YAML            (-c flag)
    |
extra/ contents            (-e flag)
    |  copied into image filesystem
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

Change one file in `--extra`, one byte of the cloud-init payload, the kernel cmdline, or the kernel binary — and the roothash changes, which changes the UKI, which changes the IGVM measurement. A remote verifier checks the launch digest against a published expected value and can trust the entire stack.

## Output Artifacts

```
output/<name>/
├── disk.raw         GPT disk image (ESP + erofs root + verity hash partitions)
├── uki.efi          Unified Kernel Image (kernel + initrd + cmdline)
├── roothash         SHA-256 hex string of the verity root
├── manifest.json    Build metadata: input hashes, output hashes, smp/memory,
│                    per-fragment shas, optional SNP measurement
├── OVMF.fd          Firmware (copy of the --firmware input; bundled here so the
│                    output dir is self-contained for `steep run` and `steep push`)
└── guest.igvm       IGVM file (absent when --skip-igvm)
```

The manifest is the authoritative description of what's in the build. To verify an image you got from elsewhere, compare `manifest.json`'s `outputs.uki.sha256` and (with IGVM) `measurement.snp_launch_digest` against the published expected values for the build inputs you trust.
