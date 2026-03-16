# steep container: OCI Container CVM Image Builder

## Overview

Implement `steep container`, which takes an OCI container image URL, bakes the image into a project partition with podman and a systemd quadlet unit, and runs the shared pipeline to produce a confidential VM image with attestation artifacts.

This builds on the existing `steep cloud-init` pipeline. The shared stages (disk composition, UKI, IGVM, format conversion, manifest) are extracted into a reusable pipeline module that both `cloud-init` and `container` call.

## Deliverables

1. Shared pipeline module — extract stages 5-9 from `cloud_init.rs` into `src/pipeline.rs`
2. Container build helpers — OCI image pull/save, quadlet generation, mkosi build tree assembly
3. `steep container` — full implementation replacing the current stub

## CLI

```
steep container <URL>
    --kernel <PATH>        # path to hardened kernel
    --initrd <PATH>        # path to base initrd (input to UKI build)
    --firmware <PATH>      # path to OVMF firmware binary
    --base-image <PATH>    # path to base image (from `steep base`)
    --service-port <PORT>  # TCP port to open through firewall (u16, required)
    --memory <SIZE>        # RAM for VM, QEMU-style suffix (default: "2G")
    --smp <N>              # number of vCPUs (default: 1)
    --format <FORMAT>      # output format: qcow2, vhd, raw (default: qcow2)
    -o, --output <DIR>     # output directory for artifacts
```

`URL` is an OCI container image reference (e.g., `ghcr.io/org/app:latest`). Tag-based references are required; digest-based references (`@sha256:...`) are not supported.

The flags match `steep cloud-init` exactly. `--service-port` and `--memory` are added to `ContainerArgs` to align the interfaces.

## Shared Pipeline Extraction

### Module: `src/pipeline.rs`

Stages 5-9 of the cloud-init pipeline are identical for both commands and are extracted into a shared module.

```rust
pub struct PipelineArgs {
    pub project_partition: PathBuf,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub firmware: PathBuf,
    pub base_image: PathBuf,
    pub service_port: u16,
    pub memory: String,
    pub smp: u32,
    pub format: ImageFormat,
    pub output: PathBuf,
}

pub fn run(args: &PipelineArgs) -> anyhow::Result<()>
```

The function runs:

- **Stage 5: Compose disk** — base + project partition via repart (calls `compose::disk::compose()`)
- **Stage 6: Build UKI** — kernel + initrd via ukify (calls `uki::build::build()`)
- **Stage 7: Build IGVM** — firmware + UKI via igvm-tools (calls `igvm::invoke::build()`)
- **Stage 8: Convert format** — raw → qcow2/vhd via qemu-img if needed (calls `convert::convert()`)
- **Stage 9: Write manifest** — hashes + measurements (calls `manifest::write_manifest()`)

`smp` is used in stage 7 (IGVM build via `IgvmBuildArgs`) and stage 9 (manifest `BuildConfig`). `memory` is used only in stage 9 (manifest `BuildConfig`) — it is not passed to igvm-tools. Neither is used by stages 5, 6, or 8.

Helper functions `hash_entry()`, `chrono_now()`, and `format_extension()` move from `cloud_init.rs` to `pipeline.rs`.

After extraction, `cloud_init::run()` handles stages 1-4 (validate, check tools, mkdir, build project partition via mkosi with the user-provided cloud-init dir), then calls `pipeline::run()`. Behavior is identical to the current implementation.

## steep container

### Container Build Steps

#### Step 1: Validate inputs

Same file-existence checks as cloud-init: kernel, initrd, firmware, base_image must exist.

#### Step 2: Check tools

Verify availability of: mkosi, ukify, igvm-tools, qemu-img, **podman**.

#### Step 3: Create output directory

`fs_err::create_dir_all(&args.output)`

#### Step 4: Build project partition

This is the container-specific stage. It creates a mkosi build tree that installs podman, loads the baked OCI image, and configures a systemd quadlet to run it.

**4a: Pull and export OCI image**

```
podman pull <URL>
podman save -o <work_dir>/container.oci <URL>
```

Both commands are run on the build host via `tools::run_command_streaming()`. The OCI archive is a tar file containing the image layers. `podman save` preserves the image's tag so that `podman load` on the target restores it with the same name.

Note: After `podman pull` and `podman save`, the pulled image remains in the build host's podman image store. Cleanup of the host store is out of scope — the user can run `podman image prune` separately if needed.

**4b: Generate mkosi build tree**

The container command creates the mkosi build tree directly — it does not use `MkosiConfig::cloud_init()` since there is no cloud-init directory. Instead, a new `MkosiConfig::container()` constructor is added that produces a minimal partition config without requiring a cloud-init dir.

`MkosiConfig::container()` produces a `mkosi.conf` with:

```ini
[Distribution]
Distribution=ubuntu

[Content]
Packages=podman

[Output]
Format=disk
```

The `Packages=podman` entry tells mkosi to install podman into the partition during the build, as an alternative to the postinst apt-get install. However, the postinst script also runs `apt-get install -y podman` as a fallback to ensure podman is available regardless of mkosi's package resolution. The `Content` section does not reference a cloud-init directory.

The `mkosi.extra/` directory tree is populated by the orchestration code in `commands/container.rs` before calling `mkosi_config.invoke()`. mkosi copies everything under `mkosi.extra/` into the image root, so `mkosi.extra/opt/steep/container.oci` becomes `/opt/steep/container.oci` in the built partition.

For the `mkosi.extra/` tree, two mechanisms are used:
- **Small files** (quadlet unit): written via `MkosiConfig::add_extra_file(relative_path, content)`, which stores the path and content in a `Vec<(PathBuf, Vec<u8>)>` field on `MkosiConfig` and writes them during `invoke()`.
- **Large files** (OCI archive): copied directly by the orchestration code via `fs_err::copy()` into the `mkosi.extra/` tree before calling `invoke()`. This avoids loading multi-gigabyte OCI archives into memory.

Create a temporary working directory with this structure:

```
work_dir/
├── mkosi.conf                                          # from MkosiConfig::container()
├── mkosi.postinst.d/
│   ├── 00-script.sh                                    # nftables::service_rules(port)
│   └── 01-script.sh                                    # install podman + load image
├── mkosi.extra/
│   ├── opt/steep/container.oci                         # baked OCI archive
│   └── etc/containers/systemd/app.container            # quadlet unit
```

Postinst script filenames are `{:02}-script.sh` as generated by `MkosiConfig::write_postinst_scripts()`. The nftables script must be added first (index 0), then the podman script (index 1), to ensure correct execution order.

**4c: Podman postinst script**:

```bash
#!/bin/bash
set -euo pipefail
apt-get install -y podman
podman load -i /opt/steep/container.oci
rm /opt/steep/container.oci
```

This runs inside the mkosi build chroot. It installs podman into the project partition, loads the OCI archive into podman's local image store (`/var/lib/containers/storage/`), and removes the archive to avoid duplicating image data in the partition. The image is then available at boot without any network pull.

The path `/opt/steep/container.oci` is a fixed convention — it matches the `mkosi.extra/opt/steep/container.oci` placement in the build tree.

mkosi preserves all filesystem changes made by postinst scripts in the output partition image, including podman's image store under `/var/lib/containers/storage/`.

**4d: Quadlet unit** (`app.container`):

```ini
[Container]
Image=<URL>
PublishPort=<service-port>:<service-port>

[Service]
Restart=always

[Install]
WantedBy=multi-user.target default.target
```

The `Image=` field references the image by its original URL/tag, which resolves to the locally loaded image (tag preserved by `podman save`/`podman load`). `PublishPort` maps the service port from host to container. The unit is enabled by default via `WantedBy`.

**4e: Invoke mkosi**

Use the new `MkosiConfig::container()` constructor, add the nftables and podman postinst scripts via `add_postinst_script()` (nftables first, podman second), and invoke mkosi to build the project partition.

#### Step 5-9: Shared pipeline

Call `pipeline::run()` with the built project partition and all other args.

### Output artifacts

Same as `steep cloud-init`:

```
output/
├── disk.{qcow2,vhd,raw}
├── guest.igvm
├── uki.efi
└── manifest.json
```

The manifest is identical in schema. The container image URL is not tracked separately in the manifest — it is captured implicitly via the project partition hash.

## Container Helpers Module

### Module: `src/container.rs`

Contains the container-specific logic, keeping `commands/container.rs` focused on orchestration:

- `pull(url: &str) -> anyhow::Result<()>` — runs `podman pull <URL>`
- `save(url: &str, dest: &Path) -> anyhow::Result<()>` — runs `podman save -o <dest> <URL>`
- `quadlet(url: &str, service_port: u16) -> String` — generates the `.container` quadlet file content
- `podman_postinst() -> String` — generates the podman install + image load postinst script. Uses the fixed path `/opt/steep/container.oci`.

## Rust Project Changes

### New files

| File | Responsibility |
|------|---------------|
| `src/pipeline.rs` | Shared pipeline stages 5-9 (compose, UKI, IGVM, convert, manifest) |
| `src/container.rs` | OCI image pull/save, quadlet generation, postinst script generation |

### Modified files

| File | Change |
|------|--------|
| `src/lib.rs` | Add `service_port: u16` and `memory: String` to `ContainerArgs`; add `pub mod pipeline;` and `pub mod container;` |
| `src/commands/container.rs` | Replace stub with full implementation |
| `src/commands/cloud_init.rs` | Replace inline stages 5-9 with `pipeline::run()` call; remove `hash_entry()`, `chrono_now()`, `format_extension()` |
| `src/mkosi/config.rs` | Add `Container` variant to `MkosiProfile`; add `MkosiConfig::container()` constructor (produces Distribution/Content with `Packages=podman`/Output sections, no cloud-init dir); add `extra_files: Vec<(PathBuf, Vec<u8>)>` field; add `add_extra_file(relative_path, content)` method; add `write_extra_files(work_dir)` method that writes to `mkosi.extra/`; update `invoke()` to call `write_extra_files()` |

### Unchanged files

| File | Reason |
|------|--------|
| `src/nftables.rs` | `service_rules()` reused as-is |
| `src/compose/disk.rs` | Called via pipeline, no changes |
| `src/uki/build.rs` | Called via pipeline, no changes |
| `src/igvm/invoke.rs` | Called via pipeline, no changes |
| `src/convert.rs` | Called via pipeline, no changes |
| `src/manifest.rs` | Called via pipeline, no changes |
| `src/source.rs` | Not used by container (no source image resolution) |
| `src/qemu.rs` | Not used during build |
| `src/commands/run.rs` | Works with any manifest, no changes needed |

### No new Rust dependencies

Podman is an external build-time tool, not a crate.

## Error Handling

Follows the existing pattern:

- Validate inputs exist before invoking tools (`fs_err` for path-aware errors)
- Check tool availability via `tools::require()` before invocation (including `podman`)
- Use `tools::run_command_streaming()` for real-time output from `podman pull` and `podman save`
- Check exit codes and surface tool errors with context
- Fail fast — no retries or fallbacks
