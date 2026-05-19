# 🍵 steep, secure VM image builder

Steep is a confidential VM image builder for AMD SEV-SNP. It produces
bit-identical, attestable disk images from declarative configuration. The `build`
command builds a dm-verity protected root filesystem, bundles it into a Unified
Kernel Image (UKI), and optionally wraps it in an IGVM for measured launch on
SNP hardware.

| Document | What it covers |
|----------|---------------|
| [Concepts](docs/CONCEPTS.md) | Architecture, boot steps, image structure to ensure security |
| [Reproducibility](docs/REPRODUCIBILITY.md.md) | Changes needed, prior art, comparisons to other work |

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

Steep runs on Ubuntu Linux. Clone the steep repo and run `bin/setup` to install everything you'll need.

```bash
git clone https://github.com/confidential-ai/steep.git
cd steep
bin/setup
```

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
