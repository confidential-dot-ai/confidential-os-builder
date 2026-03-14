# steep: Pipeline Wiring & Real Tool Invocations

## Overview

Wire up the stubbed pipeline stages in `steep` to perform real invocations of external tools (mkosi, ukify, igvm-tools, qemu-img, qemu). Add a setup script for installing dependencies, implement firewall hardening, and add a `run` subcommand for launching VMs.

This builds on the existing scaffolding which has CLI parsing, argument construction, and module structure in place but with stub implementations in the command handlers.

## Deliverables

1. `bin/setup` — bash script to install all external tool dependencies on Ubuntu
2. `steep base` — real mkosi invocation with nftables hardening (block all traffic)
3. `steep cloud-init` — full pipeline: mkosi, disk composition (repart), ukify, igvm-tools, qemu-img, manifest
4. `steep run` — launch a confidential VM with QEMU from cloud-init output artifacts

## bin/setup

A bash script that installs all external tool dependencies on Ubuntu. Idempotent and safe to re-run.

### System packages (apt)

- `mkosi` — image builder
- `systemd-ukify` — UKI construction
- `qemu-utils` — provides `qemu-img` for format conversion
- `qemu-system-x86` — QEMU for running VMs

### igvm-tools (cargo)

Two paths, preferring the local checkout:

- **If `../igvm-tools` exists:** `cargo install --path ../igvm-tools`
- **Otherwise:** `cargo install --git https://github.com/lunal-dev/igvm-tools`

### OVMF firmware

- **If `../igvm-tools` exists:** Copy `../igvm-tools/examples/prebuilt/OVMF.fd` to `~/.local/share/steep/OVMF.fd`
- **Otherwise:** Download the prebuilt OVMF from the igvm-tools GitHub repository to the same location

### Requirements

- Ubuntu (apt-based)
- Rust toolchain (for cargo install)
- sudo access (for apt)

## steep base

### CLI (updated)

```
steep base
    --source-image <PATH|URL>  # Ubuntu cloud image (local path or URL)
    -o, --output <DIR>         # output directory
```

### Source image resolution

`--source-image` accepts either a local file path or a URL:

- **Local path:** Use the file directly.
- **URL:** Download to `~/.local/share/steep/base-inputs/` and use the cached copy. If the file already exists in the cache (matched by filename), skip the download. This avoids re-downloading large cloud images on repeated builds.

### Behavior

1. Resolve `--source-image` — download and cache if URL, validate exists if local path
2. Generate mkosi config for base image (existing `MkosiConfig::base()`)
3. Write nftables hardening postinst script into the mkosi build tree — **blocks all new incoming and outgoing connections; only loopback and already-established connections are permitted**
4. Invoke `mkosi` to build the base partition
5. Output `base.raw` to the output directory

### nftables base rules

The base image includes a restrictive nftables ruleset installed via mkosi postinst script:

```nftables
#!/usr/sbin/nft -f
flush ruleset
table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
        iif "lo" accept
        ct state established,related accept
    }
    chain forward {
        type filter hook forward priority 0; policy drop;
    }
    chain output {
        type filter hook output priority 0; policy drop;
        oif "lo" accept
        ct state established,related accept
    }
}
```

This blocks everything except loopback and established connections. The cloud-init project layer opens the service port.

## steep cloud-init

### CLI changes

Add new flags:

```
steep cloud-init <DIR>
    --kernel <PATH>
    --initrd <PATH>
    --firmware <PATH>
    --base-image <PATH>
    --service-port <PORT>  # NEW (required): incoming port to allow through firewall (u16)
    --memory <SIZE>        # NEW: RAM for VM, QEMU-style suffix string (default: "2G")
    --smp <N>              # default: 1
    --format <FORMAT>      # default: qcow2
    -o, --output <DIR>
```

`--service-port` is a `u16` specifying the single TCP port to open for inbound traffic in the project partition's nftables rules.

`--memory` is a `String` passed directly to QEMU's `-m` flag (e.g., `"2G"`, `"512M"`). It is recorded in the build manifest so that `steep run` can launch the VM with the correct memory configuration.

### Pipeline stages

#### Stage 1-3: Validate, check tools, create output dir (already implemented, minor update needed)

Existing validation and tool checks remain. Tool checks verify: mkosi, ukify, igvm-tools, qemu-img. Update validation to also check the new `--service-port` and `--memory` args.

#### Stage 4: Build project partition

- Generate mkosi cloud-init config (existing `MkosiConfig::cloud_init()`)
- Add nftables postinst script that opens `--service-port`:

```nftables
#!/usr/sbin/nft -f
flush ruleset
table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;
        iif "lo" accept
        ct state established,related accept
        tcp dport <SERVICE_PORT> accept
    }
    chain forward {
        type filter hook forward priority 0; policy drop;
    }
    chain output {
        type filter hook output priority 0; policy accept;
    }
}
```

Note: The project layer's rules replace the base layer's block-all output policy with `accept`, since the service needs outbound connectivity. Inbound is restricted to the single service port plus established connections.

- Invoke `mkosi` to build the project partition

#### Stage 5: Compose disk image

Use mkosi's repart integration to assemble the final GPT disk image from two partitions. The existing `compose::disk::compose()` function becomes the entry point that orchestrates this.

Generate systemd-repart partition definition files in a temp directory:

**`00-base.conf`:**
```ini
[Partition]
Type=root
Format=ext4
CopyBlocks=<base_partition_path>
ReadOnly=yes
SizeMinBytes=2G
```

**`10-project.conf`:**
```ini
[Partition]
Type=generic
Format=ext4
CopyBlocks=<project_partition_path>
ReadOnly=yes
SizeMinBytes=512M
```

Invoke mkosi with the repart definitions directory to produce the composed GPT image. The `compose::disk::compose()` function:
1. Writes the repart `.conf` files to a temp directory
2. Invokes mkosi with the repart config pointing at the definitions
3. Outputs `disk.raw`

The `MkosiConfig` gains a new constructor `MkosiConfig::repart(definitions_dir, output)` for this purpose.

Output: `disk.raw`

#### Stage 6: Build UKI

Invoke `ukify` via the existing `uki::build` module:

- Input: kernel + initrd
- Output: `uki.efi`
- Uses `UkifyBuildArgs` and `uki::build()` (module already implemented, not yet called from the pipeline)

#### Stage 7: Build IGVM

Invoke `igvm-tools` via the existing `igvm::invoke` module:

- Input: OVMF firmware + UKI
- Flags: `--smp`, `--platform snp`, `--manifest`
- Output: `guest.igvm` + igvm-tools manifest JSON
- Uses `IgvmBuildArgs` and `igvm::build()` (module already implemented, not yet called from the pipeline)

#### Stage 8: Convert format

If output format is not `raw`, invoke `qemu-img convert`:

```
qemu-img convert -f raw -O qcow2 disk.raw disk.qcow2
qemu-img convert -f raw -O vpc disk.raw disk.vhd
```

Skip conversion if format is `raw`.

#### Stage 9: Write manifest

- Hash all input files (kernel, initrd, firmware, base image, project partition) using existing `sha256_file()`
- Hash all output files (disk image, IGVM, UKI)
- Parse igvm-tools manifest to extract SNP launch digest, page count, VMSA count
- Merge into steep's `BuildManifest` schema (extended with `memory` field in `BuildConfig`)
- Write `manifest.json` via existing `write_manifest()`

The `BuildConfig` struct gains a `memory: String` field (e.g., `"2G"`) so that `steep run` can read it back.

### Output artifacts

`uki.efi` is promoted from an intermediate artifact to a user-visible output, since it is independently useful for attestation verification and debugging.

```
output/
├── disk.{qcow2,vhd,raw}
├── guest.igvm
├── uki.efi
└── manifest.json
```

## steep run

### CLI

```
steep run <DIR>
```

No flags. All runtime parameters (smp, memory, disk format) are read from `manifest.json` in the output directory.

`DIR` is the output directory from `steep cloud-init`. The command discovers artifacts by convention:

- `manifest.json` — required, provides smp, memory, and format
- `guest.igvm` — required
- `disk.{qcow2,vhd,raw}` — format determined from manifest

### Behavior

1. Validate `DIR` exists
2. Read and parse `manifest.json` to get smp, memory, and disk format
3. Find `guest.igvm` in the directory
4. Find the disk image using the format from the manifest
5. Check `qemu-system-x86_64` is available
6. Invoke QEMU with SEV-SNP flags, the IGVM file, and the disk image

### QEMU invocation

```
qemu-system-x86_64 \
    -machine q35,confidential-guest-support=sev0,igvm-cfg=igvm0 \
    -object sev-snp-guest,id=sev0 \
    -object igvm-cfg,id=igvm0,file=<guest.igvm> \
    -drive file=<disk.qcow2>,format=qcow2,if=virtio \
    -smp <from manifest> \
    -m <from manifest> \
    -nographic
```

The exact QEMU flags for SEV-SNP + IGVM may need adjustment based on the QEMU version and SNP support available.

## Rust project changes

### New files

| File | Responsibility |
|------|---------------|
| `bin/setup` | External tool installation script |
| `src/nftables.rs` | Generate nftables rule files |
| `src/commands/run.rs` | `run` subcommand — QEMU invocation |

### Modified files

| File | Change |
|------|--------|
| `src/lib.rs` | Add `--service-port` (u16) and `--memory` (String, default "2G") to `CloudInitArgs`, add `RunArgs` struct (dir only, no flags), add `memory` to `BuildConfig`, add `run` subcommand to enum |
| `src/main.rs` | Add dispatch for `run` subcommand |
| `src/commands/mod.rs` | Export `run` module |
| `src/commands/base.rs` | Replace stub with mkosi invocation + nftables hardening |
| `src/commands/cloud_init.rs` | Replace stubs with real invocations for stages 4-9 |
| `src/mkosi/config.rs` | Extend with `add_postinst_script(content)` method for nftables injection, add `MkosiConfig::repart(definitions_dir, output)` constructor for disk composition |
| `src/compose/disk.rs` | Replace stub with mkosi repart invocation |
| `src/manifest.rs` | Add igvm-tools manifest parsing and merge logic |

### Unchanged files

| File | Reason |
|------|--------|
| `src/tools.rs` | Already complete |
| `src/uki/build.rs` | Already complete (not yet called from pipeline, will be wired in cloud_init.rs) |
| `src/igvm/invoke.rs` | Already complete (not yet called from pipeline, will be wired in cloud_init.rs) |
| `src/commands/kernel.rs` | Deferred per spec |
| `src/commands/container.rs` | Deferred per spec (Future Work) |

### New dependency

- None expected — existing dependencies cover all needs

## Error handling

Follows the existing pattern established in the codebase:

- Validate inputs exist before invoking tools (`fs_err` for path-aware errors)
- Check tool availability via `tools::require()` before invocation
- Use `tools::run_command_streaming()` for real-time output from external tools
- Check exit codes and surface tool errors with context
- Fail fast — no retries or fallbacks

## Design decisions

- **nftables over iptables:** The original design spec mentions "iptables/nftables" for firewall hardening. This implementation uses nftables exclusively — it is the modern replacement for iptables, is the default on Ubuntu, and avoids maintaining two parallel rulesets.
