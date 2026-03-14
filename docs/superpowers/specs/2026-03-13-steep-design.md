# steep: Confidential VM Image Builder

## Overview

`steep` is a Rust CLI tool that orchestrates [mkosi](https://github.com/systemd/mkosi) and [igvm-tools](https://github.com/lunal-dev/igvm-tools) to produce confidential virtual machine images for AMD SEV-SNP (with Intel TDX support planned).

The tool takes a security-hardened base image, layers a project-specific partition on top, builds a Unified Kernel Image (UKI), and invokes igvm-tools to produce an IGVM file with a deterministic SNP launch digest.

## CLI Interface

Binary name: `steep`

### Subcommands

#### `steep kernel`

Builds the security-hardened custom kernel. The kernel build details (source location, hardening config, patches) are out of scope for the initial implementation — this subcommand provides the entry point and will be fleshed out as the kernel hardening requirements are finalized. Initially, it may wrap an existing kernel build script or Makefile.

```
steep kernel
    --source <PATH>        # path to kernel source tree
    --config <PATH>        # path to kernel .config (hardening config)
    -o, --output <DIR>     # where to write kernel + initrd
```

**Outputs:** `vmlinuz` (kernel binary) + `initrd.img` (base initramfs)

#### `steep base`

Builds the security-hardened base partition from an Ubuntu cloud image.

```
steep base
    --source-image <PATH>  # Ubuntu cloud image to start from
    -o, --output <DIR>     # where to write the base partition image
```

**Phase 1 hardening:** Locked-down network firewall rules (iptables/nftables)
**Future hardening:** SELinux policy, additional OS hardening

**Outputs:** `base.raw` (base partition image)

#### `steep cloud-init <DIR>`

Builds a project-specific partition from a cloud-init configuration directory, composes the final disk image, and produces an IGVM file.

```
steep cloud-init <DIR>
    --kernel <PATH>        # path to hardened kernel
    --initrd <PATH>        # path to base initrd (input to UKI build, not passed to igvm-tools)
    --firmware <PATH>      # path to OVMF binary
    --base-image <PATH>    # path to base image (from `steep base`)
    --smp <N>              # number of vCPUs (default: 1; affects SNP launch digest)
    --format <FORMAT>      # output format: qcow2, vhd, raw (default: qcow2)
    -o, --output <DIR>     # output directory for artifacts
```

`DIR` is a standard cloud-init layout containing `user-data`, `meta-data`, and optionally `vendor-data`, `network-config`.

#### `steep container <URL>`

Builds a project-specific partition for a container workload. Generates a standard cloud-init configuration that pulls and runs the specified container image. The container subcommand is defined in the CLI from the start but its implementation details (container runtime, cloud-init modules, systemd units) will be designed when the container runtime work begins (see Future Work).

```
steep container <URL>
    --kernel <PATH>        # path to hardened kernel
    --initrd <PATH>        # path to base initrd (input to UKI build, not passed to igvm-tools)
    --firmware <PATH>      # path to OVMF binary
    --base-image <PATH>    # path to base image (from `steep base`)
    --smp <N>              # number of vCPUs (default: 1; affects SNP launch digest)
    --format <FORMAT>      # output format: qcow2, vhd, raw (default: qcow2)
    -o, --output <DIR>     # output directory for artifacts
```

`URL` is an OCI container image reference (e.g., `ghcr.io/org/app:latest`).

### Global Flags

All subcommands support:
- `-v` / `-q` — verbosity control via `clap-verbosity-flag` (maps to tracing log levels)

## Architecture

### Pipeline

```
steep kernel  → hardened kernel + initrd
steep base    → base partition image (Ubuntu + hardening)

steep cloud-init/container:
    1. Build project-specific partition (cloud-init config or container setup)
    2. Compose final disk image: base partition + project partition + GPT table
    3. (Later) Calculate dm-verity hashes for both partitions, embed in initrd
    4. Build UKI from kernel + initrd (+ verity initrd later)
    5. Invoke igvm-tools with OVMF + UKI → IGVM + launch digest
    6. Write manifest JSON
```

### Design Principle: Two-Partition Architecture

The disk image is composed of two independently-built partitions:

- **Base partition** — Ubuntu OS + security hardening. Built rarely via `steep base`.
- **Project partition** — project-specific configuration. Built per-deploy via `cloud-init` or `container` subcommands.

This separation means:
- The expensive base image build happens infrequently
- Project deploys only rebuild the lightweight project partition
- Each partition gets independent dm-verity verification (future phase)
- Both partitions are read-only at runtime; an overlay provides write access on the project partition

### Disk Layout

```
┌─────────────────────────────────────────┐
│ GPT partition table                     │
├─────────────────────────────────────────┤
│ Base partition (root fs, read-only)     │
├─────────────────────────────────────────┤
│ Project partition (read-only + overlay) │
├─────────────────────────────────────────┤
│ (Later) dm-verity hash partitions       │
└─────────────────────────────────────────┘
```

Note: No ESP is included. The UKI is injected into the IGVM via igvm-tools' HOB mechanism, not loaded from disk. The OVMF firmware discovers the kernel via HOB, not via an EFI System Partition.

The project partition is mounted read-only with an overlay (tmpfs or ephemeral) for runtime writes.

### UKI and IGVM

The UKI (Unified Kernel Image) bundles the hardened kernel + initramfs into a single PE/COFF EFI binary. The UKI is built using `systemd-ukify` (or mkosi's built-in UKI support if mkosi is managing the build). In a later phase, a second initrd containing the dm-verity root hashes for both partitions will be added to the UKI.

**Data flow:** `(kernel + initrd) → ukify → UKI.efi → igvm-tools --kernel UKI.efi`

Note: The `--initrd` CLI flag is an input to the UKI build step. It is NOT passed directly to igvm-tools. igvm-tools receives only the final UKI EFI binary via its `--kernel` flag.

The build order enforces this dependency:
1. Finalize disk image (both partitions composed)
2. (Later) Calculate dm-verity root hashes
3. Build UKI: kernel + base initrd (+ verity initrd later) → `UKI.efi`
4. Invoke igvm-tools: OVMF + UKI → IGVM file + SNP launch digest

igvm-tools is invoked as:
```
igvm-tools build \
    --firmware <OVMF> \
    --kernel <UKI.efi> \
    --smp <N> \
    --platform snp \
    --manifest <igvm-manifest.json> \
    -o <output.igvm>
```

The `--smp` flag is critical: the SNP launch digest includes one VMSA per vCPU, so the digest changes with vCPU count. The `--platform snp` is explicit (though it is the igvm-tools default). The `--boot-mode` flag is omitted, relying on the igvm-tools default of `real16` which is appropriate for OVMF SEV-SNP guests.

### Output Artifacts

```
output/
├── disk.{qcow2,vhd,raw}   # composed disk image (base + project partitions)
├── guest.igvm               # IGVM file for SEV-SNP launch
└── manifest.json            # hashes, measurements, build metadata
```

steep produces its own manifest that is a superset of the igvm-tools manifest. It calls igvm-tools with `--manifest`, parses the result, and embeds the measurement data into its richer manifest.

```json
{
  "version": 1,
  "build": {
    "timestamp": "2026-03-13T12:00:00Z",
    "smp": 4,
    "format": "qcow2",
    "platform": "snp"
  },
  "inputs": {
    "kernel": { "path": "vmlinuz", "sha256": "..." },
    "initrd": { "path": "initrd.img", "sha256": "..." },
    "firmware": { "path": "OVMF.fd", "sha256": "..." },
    "base_image": { "path": "base.raw", "sha256": "..." },
    "project_partition": { "path": "project.raw", "sha256": "..." }
  },
  "outputs": {
    "disk_image": { "path": "disk.qcow2", "sha256": "..." },
    "igvm": { "path": "guest.igvm", "sha256": "..." },
    "uki": { "path": "uki.efi", "sha256": "..." }
  },
  "measurement": {
    "snp_launch_digest": "...",
    "algorithm": "sha384",
    "page_count": 5598,
    "vmsa_count": 4
  }
}
```

### Image Formats

Raw is the internal build format. Final output is converted to the requested format:
- **qcow2** — for QEMU/KVM and on-prem deployments
- **vhd** — for Azure confidential VMs
- **raw** — for direct use or GCP

Conversion uses `qemu-img convert`.

## External Dependencies

Build-time tools that must be available on the host:
- **mkosi** — base image and project partition building. Used by `steep base` to apply hardening to the Ubuntu cloud image, and by `cloud-init`/`container` to build the project partition with cloud-init configs applied.
- **systemd-ukify** — UKI construction (kernel + initrd → PE/COFF EFI binary). Used by `cloud-init`/`container` after disk composition.
- **igvm-tools** — IGVM generation from OVMF + UKI. Used as the final step of `cloud-init`/`container`.
- **qemu-img** — disk image format conversion (raw → qcow2/vhd)

User-supplied inputs:
- Custom hardened kernel + initrd (paths)
- OVMF firmware binary (path, patched edk2 with IGVM support)
- Ubuntu cloud image (for `steep base`)

## Error Handling

**Fail fast with clear context.** Each pipeline stage:
1. Validates inputs exist before invoking external tools
2. Checks that required tools are installed (with helpful error if not)
3. Passes arguments programmatically via `std::process::Command` (no shell interpolation)
4. Streams stdout/stderr so the user sees progress
5. Checks exit codes and surfaces tool errors with context

The `fs_err` crate is used instead of `std::fs` so all filesystem errors include the relevant file path.

## Rust Project Structure

```
lunal-base/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point, clap argument parsing
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── cloud_init.rs    # cloud-init subcommand logic
│   │   ├── container.rs     # container subcommand logic
│   │   ├── kernel.rs        # kernel build subcommand
│   │   └── base.rs          # base image build subcommand
│   ├── mkosi/
│   │   ├── mod.rs
│   │   └── config.rs        # mkosi config generation
│   ├── igvm/
│   │   ├── mod.rs
│   │   └── invoke.rs        # igvm-tools invocation
│   ├── compose/
│   │   ├── mod.rs
│   │   └── disk.rs          # partition composition (base + project → final image)
│   ├── manifest.rs          # manifest JSON generation
│   └── tools.rs             # external tool discovery & invocation helpers
```

### Rust Dependencies

- `clap` (derive) — CLI argument parsing
- `clap-verbosity-flag` — `-v` / `-q` flags for log level control
- `tracing` + `tracing-subscriber` — structured logging
- `serde` + `serde_json` — manifest JSON generation
- `sha2` — SHA-256/384 hashing for manifest
- `thiserror` — structured error types
- `tempfile` — scratch directories during builds
- `fs_err` — drop-in `std::fs` replacement with filename-aware errors

## Future Work

Items tracked but not designed in detail:

- **Intel TDX support** — extend igvm-tools invocation and measurement for TDX platform
- **dm-verity** — verity hash trees for both partitions, root hashes embedded in a second initrd within the UKI
- **Blackwell multi-GPU passthrough** — CUDA drivers >= 590.48.01, kernel module support for GPU passthrough in confidential VMs
- **SELinux** — policy configuration in the base image
- **Container runtime** — standard cloud-init configuration for running OCI containers inside the VM

## Confidential VM Technology

### SEV-SNP (AMD)

The primary target platform. SEV-SNP (Secure Encrypted Virtualization - Secure Nested Paging) provides:
- Memory encryption with per-VM keys
- Integrity protection against hypervisor tampering
- Attestation via launch measurements

The IGVM file produced by igvm-tools encodes the initial guest state. The SNP launch digest (SHA-384) in the manifest can be used to verify that a running VM was launched from the expected image.

### OVMF Firmware

Requires a patched edk2 build with IGVM-aware PVALIDATE handling from https://github.com/lunal-dev/edk2 (branch: `OvmfPkg-PlatformPei-skip-pvalidate-igvm-pages`).
