# cloud-init cidata partition Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dynamic MkosiConfig-based project partition in `steep cloud-init` with a static `mkosi/cidata/` directory that builds a minimal vfat cidata partition, and remove `--service-port` from the CLI.

**Architecture:** The `cloud-init` command will invoke mkosi directly against a static `mkosi/cidata/mkosi.conf` (mirroring how `base` works), passing the user's cloud-init directory via `--extra-trees`. The base image gains `cloud-init` as a package so it can discover and apply the cidata partition at boot.

**Tech Stack:** Rust, Clap (CLI), mkosi (image build), cargo test

---

## File Map

| File | Action | What changes |
|------|--------|--------------|
| `tests/mkosi_config.rs` | Modify | Remove `test_cloud_init_config_includes_cloud_init_dir`, `test_container_config_has_no_cloud_init_dir` |
| `src/mkosi/config.rs` | Modify | Remove `MkosiProfile::CloudInit`, `MkosiConfig::cloud_init()`, `cloud_init_dir` field |
| `tests/cli.rs` | Modify | Remove two service-port tests, strip `--service-port` from four others, add rejection test |
| `src/lib.rs` | Modify | Remove `service_port` from `CloudInitArgs`, update subcommand doc |
| `src/commands/cloud_init.rs` | Modify | Replace MkosiConfig invocation with direct mkosi call |
| `mkosi/cidata/mkosi.conf` | Create | Static vfat cidata mkosi config |
| `mkosi/base/mkosi.conf` | Modify | Add `[Content] Packages=cloud-init` |
| `src/compose/disk.rs` | Modify | `project_partition_conf`: `Format=vfat`, `SizeMinBytes=8M` |

---

## Task 1: Remove CloudInit from MkosiConfig

**Files:**
- Modify: `tests/mkosi_config.rs`
- Modify: `src/mkosi/config.rs`

- [ ] **Step 1: Delete the two CloudInit-related tests from `tests/mkosi_config.rs`**

  Remove `test_cloud_init_config_includes_cloud_init_dir` (lines 4–9) and `test_container_config_has_no_cloud_init_dir` (lines 40–44). The latter tests `config.cloud_init_dir`, which will not exist after the struct change.

  The file should retain: `test_repart_config`, `test_container_config_profile`, `test_container_config_ini`, `test_add_extra_file`, `test_write_extra_files`.

- [ ] **Step 2: Run the test file to confirm remaining tests compile and pass**

  ```bash
  cargo test --test mkosi_config 2>&1
  ```
  Expected: all remaining tests pass, no compile error.

- [ ] **Step 3: Remove `MkosiProfile::CloudInit` from `src/mkosi/config.rs`**

  In the `MkosiProfile` enum (lines 5–9), delete the `CloudInit,` variant.

- [ ] **Step 4: Remove `cloud_init_dir` field from `MkosiConfig` struct**

  In the `MkosiConfig` struct (lines 12–18), delete:
  ```rust
  pub cloud_init_dir: Option<PathBuf>,
  ```

- [ ] **Step 5: Remove `MkosiConfig::cloud_init()` method**

  Delete the entire `cloud_init()` method (lines 22–46).

- [ ] **Step 6: Verify the codebase compiles**

  ```bash
  cargo build 2>&1
  ```
  Expected: compile error referencing `cloud_init` usage in `src/commands/cloud_init.rs` — this is expected and will be fixed in Task 3. If there are OTHER unexpected errors, fix them now.

- [ ] **Step 7: Commit**

  ```bash
  git add src/mkosi/config.rs tests/mkosi_config.rs
  git commit -m "remove: MkosiConfig::cloud_init() and CloudInit profile"
  ```

---

## Task 2: Remove `--service-port` from `CloudInitArgs`

**Files:**
- Modify: `tests/cli.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Delete the two service-port CLI tests from `tests/cli.rs`**

  Remove `test_cloud_init_requires_service_port` (lines 133–153) and `test_cloud_init_accepts_service_port` (lines 155–176) entirely.

- [ ] **Step 2: Add a replacement test that verifies `--service-port` is now rejected**

  Add after the deleted tests:

  ```rust
  #[test]
  fn test_cloud_init_rejects_service_port() {
      let mut cmd = Command::cargo_bin("steep").unwrap();
      cmd.args([
          "cloud-init",
          "/tmp",
          "--kernel", "/tmp/k",
          "--firmware", "/tmp/f",
          "--base-image", "/tmp/b",
          "--service-port", "443",
          "-o", "/tmp/o",
      ])
      .assert()
      .failure()
      .stderr(predicates::str::contains("service-port"));
  }
  ```

- [ ] **Step 3: Strip `--service-port "443"` from the four remaining cloud-init tests**

  In each of the following tests, remove the two lines `"--service-port",` and `"443",`:
  - `test_cloud_init_fails_with_missing_dir` (around line 74–75)
  - `test_smp_default_is_one` (around line 99–100)
  - `test_format_flag_accepts_vhd` (around line 122–123)
  - `test_cloud_init_memory_default` (around line 193–194)

- [ ] **Step 4: Remove `service_port` from `CloudInitArgs` in `src/lib.rs`**

  Delete these three lines from `CloudInitArgs` (around lines 59–61):
  ```rust
  /// Single TCP port to allow through firewall
  #[arg(long)]
  pub service_port: u16,
  ```

- [ ] **Step 5: Update the `cloud-init` subcommand doc string in `src/main.rs`**

  Change:
  ```rust
  /// Build a CVM image with cloud-init configuration
  CloudInit(CloudInitArgs),
  ```
  To:
  ```rust
  /// Build a CVM image with cloud-init configuration. The cloud-init user-data must configure any required firewall rules (e.g. opening a service port with nftables).
  CloudInit(CloudInitArgs),
  ```

- [ ] **Step 6: Note on running `test_cloud_init_rejects_service_port`**

  Do NOT run `test_cloud_init_rejects_service_port` until Step 4 of this task has been applied (i.e., `service_port` is removed from `CloudInitArgs`). Before that, clap still accepts `--service-port`, so the command fails at validation ("kernel not found") rather than arg parsing — stderr will not contain "service-port" and the test will fail for the wrong reason.

  After Step 4 (and once the build compiles — it may not until Task 3 is done), run:
  ```bash
  cargo test --test cli test_cloud_init_rejects_service_port 2>&1
  ```
  Expected: PASS (clap rejects the unknown flag, stderr contains "service-port").

- [ ] **Step 7: Commit**

  ```bash
  git add src/lib.rs src/main.rs tests/cli.rs
  git commit -m "remove: --service-port from cloud-init subcommand"
  ```

---

## Task 3: Rewrite `cloud_init.rs` to invoke mkosi directly

**Files:**
- Modify: `src/commands/cloud_init.rs`

The new implementation mirrors `src/commands/base.rs`. The key differences: the mkosi directory is `mkosi/cidata`, and `--extra-trees <args.dir>` is appended to the mkosi invocation so the user's cloud-init files land at the image root.

- [ ] **Step 1: Replace the contents of `src/commands/cloud_init.rs`**

  ```rust
  use std::path::{Path, PathBuf};

  use crate::{tools, CloudInitArgs};
  use crate::pipeline::{self, PipelineArgs};

  pub fn run(args: &CloudInitArgs) -> anyhow::Result<()> {
      tracing::info!(dir = %args.dir.display(), "building cloud-init CVM image");

      // Stage 1: Validate inputs
      ensure_dir_exists(&args.dir, "cloud-init directory")?;
      ensure_file_exists(&args.kernel, "kernel")?;
      if let Some(initrd) = &args.initrd {
          ensure_file_exists(initrd, "initrd")?;
      }
      ensure_file_exists(&args.firmware, "firmware")?;
      ensure_file_exists(&args.base_image, "base image")?;

      // Stage 2: Check required tools
      tools::require("mkosi")?;
      if args.initrd.is_some() {
          tools::require("ukify")?;
      }
      tools::require("igvm-tools")?;
      tools::require("qemu-img")?;

      // Stage 3: Create output directory
      fs_err::create_dir_all(&args.output)?;

      tracing::info!("all inputs validated and tools found");

      // Stage 4: Build cidata partition via mkosi
      let mkosi_dir = PathBuf::from("mkosi/cidata");
      if !mkosi_dir.exists() {
          anyhow::bail!("mkosi config dir not found: {}", mkosi_dir.display());
      }

      let work_dir = tempfile::tempdir()?;
      tracing::info!(config = %mkosi_dir.display(), "invoking mkosi for cidata partition");
      tools::run_command_streaming("mkosi", &[
          "--directory",
          mkosi_dir.to_str().unwrap(),
          "--output-dir",
          work_dir.path().to_str().unwrap(),
          "--extra-trees",
          args.dir.to_str().unwrap(),
          "build",
      ])?;

      let project_partition = work_dir.path().join("image.raw");
      tracing::info!("cidata partition built");

      // Stages 5–9: Shared pipeline
      pipeline::run(&PipelineArgs {
          project_partition,
          kernel: args.kernel.clone(),
          initrd: args.initrd.clone(),
          firmware: args.firmware.clone(),
          base_image: args.base_image.clone(),
          memory: args.memory.clone(),
          smp: args.smp,
          format: args.format.clone(),
          output: args.output.clone(),
      })
  }

  fn ensure_file_exists(path: &Path, label: &str) -> anyhow::Result<()> {
      if !path.exists() {
          anyhow::bail!("{label} not found: {}", path.display());
      }
      if !path.is_file() {
          anyhow::bail!("{label} is not a file: {}", path.display());
      }
      Ok(())
  }

  fn ensure_dir_exists(path: &Path, label: &str) -> anyhow::Result<()> {
      if !path.exists() {
          anyhow::bail!("{label} not found: {}", path.display());
      }
      if !path.is_dir() {
          anyhow::bail!("{label} is not a directory: {}", path.display());
      }
      Ok(())
  }
  ```

- [ ] **Step 2: Verify `nftables` is not referenced in the new file**

  ```bash
  grep "nftables" src/commands/cloud_init.rs && echo "ERROR: nftables reference found" || echo "OK"
  ```
  Expected: `OK` — no output from grep.

- [ ] **Step 3: Verify the project compiles cleanly**

  ```bash
  cargo build 2>&1
  ```
  Expected: success, no warnings about unused imports or dead code.

- [ ] **Step 3: Run the full test suite**

  ```bash
  cargo test 2>&1
  ```
  Expected: all tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add src/commands/cloud_init.rs
  git commit -m "feat: invoke mkosi directly for cidata partition in cloud-init command"
  ```

---

## Task 4: Add static `mkosi/cidata/mkosi.conf`

**Files:**
- Create: `mkosi/cidata/mkosi.conf`

- [ ] **Step 1: Create the directory and config file**

  Create `mkosi/cidata/mkosi.conf`:
  ```ini
  [Output]
  Format=vfat
  Label=cidata
  Output=image.raw
  ```

  `Distribution=` is intentionally absent — mkosi does not require a distro to produce a vfat image. `Output=image.raw` documents intent; mkosi v12 always writes `image.raw` regardless.

- [ ] **Step 2: Commit**

  ```bash
  git add mkosi/cidata/mkosi.conf
  git commit -m "feat: add static mkosi config for cidata partition"
  ```

---

## Task 5: Install `cloud-init` in the base image

**Files:**
- Modify: `mkosi/base/mkosi.conf`

- [ ] **Step 1: Add `[Content]` section to `mkosi/base/mkosi.conf`**

  The current file:
  ```ini
  [Distribution]
  Distribution=ubuntu

  [Output]
  Format=disk
  Output=image.raw
  ```

  Updated file:
  ```ini
  [Distribution]
  Distribution=ubuntu

  [Content]
  Packages=cloud-init

  [Output]
  Format=disk
  Output=image.raw
  ```

- [ ] **Step 2: Commit**

  ```bash
  git add mkosi/base/mkosi.conf
  git commit -m "feat: install cloud-init in base image"
  ```

---

## Task 6: Update `project_partition_conf` for vfat cidata

**Files:**
- Modify: `src/compose/disk.rs`

- [ ] **Step 1: Update `project_partition_conf`**

  In `src/compose/disk.rs`, change `project_partition_conf` (lines 19–29) from:
  ```rust
  pub fn project_partition_conf(project_partition: &Path) -> String {
      format!(
          "[Partition]\n\
           Type=generic\n\
           Format=ext4\n\
           CopyBlocks={}\n\
           ReadOnly=yes\n\
           SizeMinBytes=512M\n",
          project_partition.display()
      )
  }
  ```
  To:
  ```rust
  pub fn project_partition_conf(project_partition: &Path) -> String {
      format!(
          "[Partition]\n\
           Type=generic\n\
           Format=vfat\n\
           CopyBlocks={}\n\
           ReadOnly=yes\n\
           SizeMinBytes=8M\n",
          project_partition.display()
      )
  }
  ```

- [ ] **Step 2: Run the full test suite**

  ```bash
  cargo test 2>&1
  ```
  Expected: all tests pass. If `tests/compose.rs` asserts on the old `ext4` or `512M` strings, update those assertions to `vfat` and `8M`.

- [ ] **Step 3: Commit**

  ```bash
  git add src/compose/disk.rs
  git commit -m "feat: update project partition to vfat/8M for cidata"
  ```

---

## Final Verification

- [ ] **Run the full test suite one last time**

  ```bash
  cargo test 2>&1
  ```
  Expected: all tests pass, no warnings.

- [ ] **Verify `nftables` is not referenced anywhere in `cloud_init.rs`**

  ```bash
  grep "nftables" src/commands/cloud_init.rs && echo "ERROR" || echo "OK"
  ```
  Expected: `OK`.

- [ ] **Verify `mkosi/cidata/mkosi.conf` has correct content**

  ```bash
  cat mkosi/cidata/mkosi.conf
  ```
  Expected: contains `Format=vfat`, `Label=cidata`, `Output=image.raw`, no `Distribution=` line.

- [ ] **Verify `--service-port` is fully gone from `steep cloud-init` help**

  ```bash
  cargo run -- cloud-init --help 2>&1
  ```
  Expected: no mention of `--service-port`. Help text should reference configuring firewall rules in user-data.

- [ ] **Verify `--service-port` is still present in `steep container --help`**

  ```bash
  cargo run -- container --help 2>&1
  ```
  Expected: `--service-port` still listed.
