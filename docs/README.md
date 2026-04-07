# 🍵 steep, secure VM image builder

## Running steep-built VMs

You can use the base images built by `steep` without even installing it yourself.

```
mkdir steep-base; cd steep-base
oras pull ghcr.io/lunal-dev/steep/base:latest
qemu-system-x86_64 -machine q35 -drive if=pflash,format=raw,readonly=on,file=OVMF.fd -kernel uki.efi -drive file=disk.raw,format=raw,if=virtio -smp 1 -m 4G -nographic
```

## Installation

Steep runs on Ubuntu Linux. Clone the steep repo and run `bin/setup` to install everything you'll need.

```bash
git clone https://github.com/confidential-ai/steep.git
cd steep
bin/setup
```

## Usage

### 1. Build the base image

```bash
steep base
```

Steep uses mkosi to build a base image for Ubuntu 26.04 (Resolute Raccoon).

### 2. Build a cloud-init image

```bash
steep cloud-init path/to/cloud-init-dir
```

This will `output/cloud-init-dir`, containing a cidata ISO disk image of the
cloud-init files, and a qcow2 copy-on-write VM image backed by the base image
generated in step 1.

### 3. Launch the built VM

```bash
steep run output/cloud-init-dir
```

Use qemu to boot the VM image with a software-emulated TPM and the cidata ISO
attached. Will run the cloud-init and update the cloud-init image into an
artifact that can be used to boot cloud VMs in the future.
