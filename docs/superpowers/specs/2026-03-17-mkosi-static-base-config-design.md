# Design: Static mkosi Config Folder for `base` Subcommand

**Date:** 2026-03-17
**Scope:** `steep base` subcommand only

## Background

The `steep base` command builds a hardened Ubuntu base partition image using mkosi. Currently, the mkosi configuration is generated at runtime by Rust code in `src/mkosi/config.rs` (`MkosiConfig::base()`), which writes a `mkosi.conf` and a nftables postinst script to a temp directory before invoking mkosi.

The base image config is entirely static — the same every time. There is no runtime-variable content. The goal is to replace the generated config with a committed folder of static mkosi config files that mkosi is invoked against directly.

## Approach

Option B: static folder in the repo, invoke mkosi with `--directory` pointing at the source folder and `--output-dir` pointing at a temp directory.

This avoids copying files at runtime while keeping the config fully visible and editable in the repo.

## Files to Create

### `mkosi/base/mkosi.conf`

```ini
[Distribution]
Distribution=ubuntu

[Output]
Format=disk
Output=image.raw
```

### `mkosi/base/mkosi.postinst.d/00-nftables.sh`

The exact content currently returned by `nftables::base_rules()`. The shebang is `#!/usr/sbin/nft -f`, making this an nft rules file executed directly. Must be committed with executable permissions (`chmod +x`).

## Code Changes

### `src/commands/base.rs`

Replace the `MkosiConfig`-based implementation with a direct mkosi invocation:

- Drop imports of `crate::mkosi::config::MkosiConfig` and `crate::nftables`
- Locate `./mkosi/base` relative to the current working directory (tool is always run from repo root)
- Bail with a clear error if the directory does not exist
- Create a temp directory for mkosi output
- Invoke: `mkosi --directory <mkosi/base> --output-dir <tempdir> build`
- Copy `<tempdir>/image.raw` to `args.output/base.raw`

### `src/mkosi/config.rs`

- Remove `MkosiProfile::Base` variant from the `MkosiProfile` enum
- Remove `MkosiConfig::base()` constructor method

All other methods and variants (`CloudInit`, `Container`, `Repart`, postinst/extra-file helpers, `invoke`) are unchanged. The module remains in use by `container`, `cloud_init`, and `compose/disk.rs`.

### `src/nftables.rs`

- Remove `base_rules()` function

`service_rules()` remains in use by `container` and `cloud_init` and is untouched.

### `tests/mkosi_config.rs`

Remove the four tests that reference `MkosiConfig::base()` or `MkosiProfile::Base`:

- `test_base_config_generates_valid_ini`
- `test_config_profile`
- `test_add_postinst_script`
- `test_invoke_args_base`

All remaining tests (container config, extra-file writing, etc.) are unchanged.

## Out of Scope

- `container`, `cloud_init`, and `repart` mkosi config generation — these remain Rust-generated for now
- Adding a CLI flag to override the mkosi config directory path

## Verification

After the change:

1. `cargo build` compiles without errors or warnings
2. `cargo test` passes (no references to removed items)
3. `steep base --output ./output` runs successfully when invoked from the repo root and `mkosi/base/` is present
