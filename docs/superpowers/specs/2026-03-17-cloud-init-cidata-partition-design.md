# Design: cloud-init cidata partition via static mkosi config

**Date:** 2026-03-17

## Summary

Replace the dynamic `MkosiConfig`-based project partition build in `steep cloud-init` with a static mkosi config directory (`mkosi/cidata/`) that builds a minimal vfat cidata partition. The user's cloud-init directory is passed at runtime via `--extra-trees`. Remove `--service-port` from the CLI; users are responsible for opening ports in their `user-data`. Install `cloud-init` in the base image so it can discover and apply the cidata partition at boot.

## Motivation

The current `cloud-init` subcommand uses mkosi to build a full Ubuntu project partition — an unnecessary overhead for what is essentially a two-file data partition. It also codifies firewall config (`--service-port`) at the `steep` level, when that policy belongs in the user's cloud-init `user-data`. The base image does not currently include `cloud-init`, so the cidata partition is never actually applied at boot.

## Changes

### `mkosi/base/mkosi.conf` — install cloud-init in base image

Add a new `[Content]` section (the current file has only `[Distribution]` and `[Output]`):

```ini
[Content]
Packages=cloud-init
```

The existing default-deny nftables rules in `mkosi.postinst.d/00-nftables.sh` remain unchanged. Cloud-init applies its own ruleset from `user-data` at first boot, replacing the base default-deny rules (both the base script and any user-data nftables config use `flush ruleset` before applying rules).

### `mkosi/cidata/mkosi.conf` — new static config directory

New checked-in file. `Distribution=` is intentionally omitted — mkosi does not need to bootstrap a distro to produce a vfat image, and omitting it avoids any package installation attempt:

```ini
[Output]
Format=vfat
Label=cidata
Output=image.raw
```

`Output=image.raw` matches the base pattern. Note: mkosi v12 ignores the `Output=` field for `mkosi build` and always writes `image.raw` in the output directory (a known quirk already handled in `compose/disk.rs`). Setting it explicitly documents intent and is consistent with `mkosi/base/mkosi.conf`.

### `src/commands/cloud_init.rs` — invoke mkosi directly

Replace the `MkosiConfig::cloud_init()` + nftables postinst approach with a direct mkosi invocation, following the same pattern as `commands/base.rs`:

```
mkosi --directory mkosi/cidata --output-dir <tempdir> --extra-trees <args.dir> build
```

- Check that `mkosi/cidata` exists before invoking mkosi, mirroring the guard in `commands/base.rs`
- `<args.dir>` is the user's cloud-init directory; mkosi copies its top-level contents into the image root. `user-data` and `meta-data` must exist at the top level of this directory — cloud-init's NoCloud datasource requires them at the filesystem root.
- Output is `image.raw` in the temp dir (see mkosi v12 note above), copied to the pipeline as `project_partition`
- Remove the `nftables::service_rules()` call and `--service-port` usage

### `src/lib.rs` — remove `--service-port`, update help

- Remove `service_port: u16` from `CloudInitArgs`
- Update the `cloud-init` subcommand doc string:
  > "Build a CVM image with cloud-init configuration. The cloud-init user-data must configure any required firewall rules (e.g. opening a service port with nftables)."

### `src/mkosi/config.rs` — remove CloudInit profile

- Remove `MkosiProfile::CloudInit`
- Remove `MkosiConfig::cloud_init()`
- Remove `cloud_init_dir: Option<PathBuf>` field from `MkosiConfig`

### `src/compose/disk.rs` — update cidata partition definition

In `project_partition_conf`:
- Change `Format=ext4` to `Format=vfat` to match the actual filesystem in the cidata image
- Reduce `SizeMinBytes` from `512M` to `8M` — the cidata partition holds only a few YAML files

`CopyBlocks=` copies the raw vfat image verbatim into the GPT slot, preserving the filesystem label `cidata`. Cloud-init discovers the partition at boot by this filesystem label, not the GPT partition type. No change to `Type=generic` or the `CopyBlocks=` directive is needed. Repart pads the partition slot to `SizeMinBytes` if the source image is smaller.

### `src/nftables.rs` — no change

`service_rules()` remains; it is still used by `steep container`.

## Data flow

```
steep cloud-init <dir> ...
  │
  ├─ mkosi --directory mkosi/cidata --extra-trees <dir> → cidata.raw
  │     (vfat, label=cidata, contains meta-data + user-data)
  │
  └─ pipeline:
       compose (base.raw + cidata.raw → disk.raw via repart)
       → ukify → igvm-tools → qemu-img → manifest
```

At boot, the base system (Ubuntu + cloud-init) discovers the cidata partition by filesystem label and applies `user-data`.

## What is NOT changed

- `steep container` — uses `MkosiConfig::container()`, which is unaffected.
- `steep base` — unaffected.
- `nftables.rs` — `service_rules()` kept for container command.
- The shared pipeline stages (ukify, igvm-tools, repart, qemu-img, manifest) — unaffected.

## Testing

- Remove tests for `MkosiConfig::cloud_init()` and `MkosiProfile::CloudInit` from `tests/mkosi_config.rs`.
- Update six tests in `tests/cli.rs` that pass `--service-port` to `steep cloud-init`:
  - `test_cloud_init_fails_with_missing_dir`
  - `test_smp_default_is_one`
  - `test_format_flag_accepts_vhd`
  - `test_cloud_init_requires_service_port` — remove entirely (flag no longer exists)
  - `test_cloud_init_accepts_service_port` — remove entirely
  - `test_cloud_init_memory_default`
- Verify `steep cloud-init` no longer requires or accepts `--service-port`.
- Verify the cidata partition produced by mkosi contains the expected files at the image root.
- Verify the base image build includes the `cloud-init` package.
