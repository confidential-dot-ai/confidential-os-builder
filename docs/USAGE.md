# steep Usage Guide

## Prerequisites

```bash
bin/setup
```

This installs mkosi, igvm-tools via cargo, and copies the prebuilt OVMF firmware
to `~/.local/share/steep/OVMF.fd`.

## Pipeline

### 1. Build the base image

```bash
steep base
```

Steep uses mkosi to build a base image for Ubuntu 26.04 (Resolute Raccoon).

### 2. Build a cloud-init image

```bash
steep cloud-init path/to/cloud-init-dir
```

Create `output/cloud-init-dir`, containing a cidata ISO disk image of the
cloud-init files, and a qcow2 copy-on-write VM image backed by the base image
generated in step 1.

### 3. Launch the VM

```bash
steep run output/cloud-init-dir
```

Use qemu to boot the VM image with a software-emulated TPM and the cidata ISO
attached. Will run the cloud-init and update the cloud-init image into an
artifact that can be used to boot cloud VMs in the future.
