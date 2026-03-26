# steep Usage Guide

## Prerequisites

Install all external tool dependencies:

```bash
bin/setup
```

This installs mkosi, igvm-tools via cargo, and copies the prebuilt OVMF firmware to `~/.local/share/steep/OVMF.fd`.

## Pipeline

### 1. Build the base image

```bash
steep base
```

Steep uses mkosi to build a base image for Ubuntu 26.04 (Resolute Raccoon)

### 2. Prepare a cloud-init directory

Create a standard cloud-init layout:

```bash
mkdir -p my-cloud-init

cat > my-cloud-init/meta-data <<'EOF'
instance-id: test-instance
local-hostname: steep-test
EOF

cat > my-cloud-init/user-data <<'EOF'
#cloud-config
packages:
  - nginx
runcmd:
  - systemctl enable --now nginx
EOF
```

### 3. Build the CVM image

```bash
steep cloud-init my-cloud-init
```

This runs the full pipeline:

1. Builds the project partition via mkosi (with nftables rules opening port 80)
2. Composes the GPT disk image (base + project partitions via repart)
3. Builds a UKI from the kernel + initrd via ukify
4. Produces an IGVM file with SNP launch digest via igvm-tools
5. Converts the disk image to the requested format (qcow2 by default)
6. Writes `manifest.json` with input/output hashes and SNP measurement

Output:

```
output/cvm/
├── disk.qcow2
├── guest.igvm
├── uki.efi
└── manifest.json
```

### 4. Inspect the manifest

```bash
cat output/cvm/manifest.json | python3 -m json.tool
```

The manifest includes SHA-256 hashes of all inputs and outputs, the SNP launch digest (SHA-384), vCPU count, memory size, and disk format.

### 5. Launch the VM

```bash
steep run output/cvm
```

Reads smp, memory, and format from `manifest.json` and launches QEMU with SEV-SNP flags. Requires SEV-SNP capable hardware and a QEMU build with IGVM support.

## Quick smoke tests

These work without external tools installed:

```bash
# Run all unit and integration tests
cargo test

# Verify CLI structure
cargo run -- --help
cargo run -- cloud-init --help
cargo run -- run --help

# Verify validation catches missing inputs
cargo run -- run /tmp
```

## Notes

- The kernel build (`steep kernel`) is not yet implemented. Supply your own hardened kernel and initrd.
- The container subcommand (`steep container`) is planned for future work.
- The `--service-port` flag is required on `cloud-init`. It opens a single TCP port in the project partition's firewall. All other inbound traffic is blocked.
- The `--memory` flag defaults to `2G` and is recorded in the manifest so `steep run` uses the same value.
- The SNP launch digest changes with `--smp` count. Use the same value for attestation verification.
