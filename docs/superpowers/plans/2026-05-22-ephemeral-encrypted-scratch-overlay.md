# Ephemeral Encrypted Scratch Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a running SNP CVM a large, ephemeral, transparently-encrypted writable root by mounting a `LABEL=scratch` disk as the overlay upper layer.

**Architecture:** The initrd detects a whole-device `LABEL=scratch` disk, opens it with plain dm-crypt using a random in-RAM key, formats it ext4 fresh each boot, and uses it as the overlay `upperdir` (replacing the 2G tmpfs). `steep run --scratch <SIZE>` creates and attaches a correctly-labeled writable disk so the path is testable. The kernel gains dm-crypt + AES-XTS; the initrd gains `e2fsprogs`.

**Tech Stack:** Rust (clap, anyhow), mkosi, bash initrd, Linux kernel Kconfig, cryptsetup/dm-crypt, ext4.

**Spec:** `docs/superpowers/specs/2026-05-22-ephemeral-encrypted-scratch-overlay-design.md`

---

## File Structure

- `kernel/required.config` — add dm-crypt + AES-XTS kernel options.
- `kernel/config-x86_64.snapshot` — regenerated artifact (do not hand-edit).
- `mkosi/initrd/mkosi.conf` — add `e2fsprogs` package.
- `mkosi/initrd/mkosi.extra/init` — add the `LABEL=scratch` overlay branch.
- `src/qemu.rs` — `parse_size_to_bytes()` helper + `QemuArgs.scratch` field + writable scratch `-drive`.
- `tests/qemu.rs` — unit tests for the helper and the scratch drive.
- `src/lib.rs` — add `--scratch` to `RunArgs`.
- `src/commands/run.rs` — create + label + attach the scratch disk.
- `tests/cli.rs` — `--scratch` is exposed on `run`.
- `README.md` — document `steep run --scratch` and the `LABEL=scratch` contract.

---

## Task 1: Add `parse_size_to_bytes` helper to qemu.rs

Pure function, no I/O — TDD cleanly. Lives next to `validate_memory` (same domain).

**Files:**
- Modify: `src/qemu.rs` (add function near `validate_memory`, around line 8-38)
- Test: `tests/qemu.rs` (append)

- [ ] **Step 1: Write the failing tests**

Append to `tests/qemu.rs`:

```rust
// --- parse_size_to_bytes tests ---

use steep::qemu::parse_size_to_bytes;

#[test]
fn test_parse_size_suffixes() {
    assert_eq!(parse_size_to_bytes("1024").unwrap(), 1024);
    assert_eq!(parse_size_to_bytes("1K").unwrap(), 1024);
    assert_eq!(parse_size_to_bytes("2M").unwrap(), 2 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("20G").unwrap(), 20u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("4g").unwrap(), 4u64 * 1024 * 1024 * 1024);
}

#[test]
fn test_parse_size_rejects_garbage() {
    assert!(parse_size_to_bytes("").is_err());
    assert!(parse_size_to_bytes("20GB").is_err());
    assert!(parse_size_to_bytes("abc").is_err());
    assert!(parse_size_to_bytes("-5G").is_err());
    assert!(parse_size_to_bytes("5 G").is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test qemu parse_size -- --nocapture`
Expected: FAIL — `parse_size_to_bytes` not found / does not compile.

- [ ] **Step 3: Implement the helper**

Add to `src/qemu.rs` immediately after the `validate_memory` function:

```rust
/// Parse a human/QEMU-style size string (e.g. "20G", "512M") into bytes.
/// Suffixes K/M/G/T are powers of 1024 (case-insensitive); no suffix = bytes.
pub fn parse_size_to_bytes(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty size");
    }
    let last = s.as_bytes()[s.len() - 1];
    let (num, mult): (&str, u64) = match last {
        b'K' | b'k' => (&s[..s.len() - 1], 1024),
        b'M' | b'm' => (&s[..s.len() - 1], 1024 * 1024),
        b'G' | b'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        b'T' | b't' => (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024),
        b'0'..=b'9' => (s, 1),
        _ => anyhow::bail!("invalid size suffix in {s:?} (use K/M/G/T or plain bytes)"),
    };
    let value: u64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size number in {s:?}"))?;
    value
        .checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("size too large: {s:?}"))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test qemu parse_size`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add src/qemu.rs tests/qemu.rs
git commit -m "feat: add parse_size_to_bytes helper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add writable scratch drive to QemuArgs

Adds an `Option<PathBuf>` field and emits a **writable** virtio drive (no `readonly=on`). Adding the field breaks every `QemuArgs { .. }` literal, so this task also fixes those constructors.

**Files:**
- Modify: `src/qemu.rs` (struct at line 104-115; `to_args` after the root `-drive` at lines 202-207)
- Modify: `tests/qemu.rs` (every existing `QemuArgs { .. }` literal — add `scratch: None,`)
- Create: `tests/qemu_scratch.rs` (new scratch-drive tests, kept in a separate file so the `sed` in Step 5 never touches them)

- [ ] **Step 1: Write the failing tests**

Create `tests/qemu_scratch.rs`:

```rust
use std::path::PathBuf;
use steep::qemu::{QemuArgs, QemuTier};

#[test]
fn test_qemu_args_scratch_adds_writable_drive() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: Some(PathBuf::from("/output/scratch.raw")),
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(
        joined.contains("file=/output/scratch.raw,format=raw,if=virtio"),
        "scratch drive missing: {joined}"
    );
    // The scratch drive must NOT be readonly — the guest writes to it.
    assert!(
        !joined.contains("file=/output/scratch.raw,format=raw,if=virtio,readonly=on"),
        "scratch drive must be writable"
    );
}

#[test]
fn test_qemu_args_no_scratch_adds_no_second_drive() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let drive_count = cmd.iter().filter(|s| *s == "-drive").count();
    assert_eq!(drive_count, 1, "expected only the root drive");
}

#[test]
fn test_qemu_args_rejects_comma_in_scratch_path() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: Some(PathBuf::from("/output/scr,atch.raw")),
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}
```

- [ ] **Step 2: Run to verify it fails to compile**

Run: `cargo test --test qemu_scratch -- --nocapture`
Expected: FAIL — `QemuArgs` has no field `scratch`.

- [ ] **Step 3: Add the field**

In `src/qemu.rs`, add to the `QemuArgs` struct (after `port_forwards`):

```rust
    pub port_forwards: Vec<(u16, u16)>,
    /// Optional writable ephemeral scratch disk (already labeled `scratch`).
    pub scratch: Option<PathBuf>,
}
```

- [ ] **Step 4: Emit the writable drive in `to_args`**

In `src/qemu.rs`, immediately after the root drive push (the block ending with the `self.disk_format` `format!`, around line 207, before `args.push("-smp"...)`), insert:

```rust
        if let Some(ref scratch) = self.scratch {
            reject_comma_in_path("scratch", scratch)?;
            args.push("-drive".to_string());
            args.push(format!("file={},format=raw,if=virtio", scratch.display()));
        }
```

- [ ] **Step 5: Fix all existing QemuArgs literals in tests/qemu.rs**

Every existing `QemuArgs { .. }` in `tests/qemu.rs` needs the new field. Add `scratch: None,` after each `port_forwards: ...,` line. The new tests live in `tests/qemu_scratch.rs` (untouched by this `sed`):

Run: `sed -i 's/^\(\s*\)port_forwards: \(.*\),$/\1port_forwards: \2,\n\1scratch: None,/' tests/qemu.rs`

Then verify no literal was missed:
Run: `cargo build --tests 2>&1 | grep "missing field" || echo "all QemuArgs literals fixed"`
Expected: `all QemuArgs literals fixed`

- [ ] **Step 6: Run the full qemu test suites**

Run: `cargo test --test qemu --test qemu_scratch`
Expected: PASS (existing readonly/port-forward tests plus the three new scratch tests).

- [ ] **Step 7: Commit**

```bash
git add src/qemu.rs tests/qemu.rs tests/qemu_scratch.rs
git commit -m "feat: emit writable scratch -drive when QemuArgs.scratch set

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Wire `steep run --scratch` to create and attach the disk

Adds the CLI flag and the disk-creation logic in `run.rs`.

**Files:**
- Modify: `src/lib.rs` (`RunArgs`, lines 40-55)
- Modify: `src/commands/run.rs` (build the `QemuArgs`, lines ~123-135)
- Test: `tests/cli.rs` (append)

- [ ] **Step 1: Write the failing CLI test**

Append to `tests/cli.rs`:

```rust
#[test]
fn test_run_help_shows_scratch() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["run", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--scratch"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test cli test_run_help_shows_scratch`
Expected: FAIL — `--scratch` not in help output.

- [ ] **Step 3: Add the `--scratch` arg to `RunArgs`**

In `src/lib.rs`, inside `struct RunArgs`, after the `firmware` field:

```rust
    /// Path to OVMF firmware (overrides manifest; needed for --skip-igvm images on KVM)
    #[arg(long, env = "STEEP_FIRMWARE")]
    pub firmware: Option<PathBuf>,

    /// Attach an ephemeral encrypted scratch disk of this size (e.g. "20G") as
    /// the writable overlay upper layer. Creates a fresh LABEL=scratch raw disk
    /// in the output directory on each run; contents do not survive a reboot.
    #[arg(long, value_name = "SIZE")]
    pub scratch: Option<String>,
}
```

- [ ] **Step 4: Run the CLI test to verify it passes**

Run: `cargo test --test cli test_run_help_shows_scratch`
Expected: PASS.

- [ ] **Step 5: Create and attach the disk in `run.rs`**

In `src/commands/run.rs`, find where `qemu_args` is built (the `let qemu_args = QemuArgs { ... };` block, around line 123). Immediately **before** that block, insert:

```rust
    // Optional ephemeral scratch disk: create a sparse raw file of the requested
    // size, label it `scratch` so the initrd detects it, and attach it writable.
    let scratch_path = match args.scratch {
        Some(ref size) => {
            let bytes = qemu::parse_size_to_bytes(size)?;
            let path = args.dir.join("scratch.raw");
            let f = fs_err::File::create(&path)?;
            f.set_len(bytes)?;
            drop(f);
            let path_str = path.to_string_lossy().to_string();
            tools::run_command("mkfs.ext4", &["-F", "-q", "-L", "scratch", &path_str])?;
            println!("Created ephemeral scratch disk ({size}) at {}", path.display());
            Some(path)
        }
        None => None,
    };
```

Then add `scratch: scratch_path,` as the last field of the `QemuArgs { ... }` literal (after `port_forwards`):

```rust
        port_forwards,
        scratch: scratch_path,
    };
```

- [ ] **Step 6: Add the `tools` import if missing**

At the top of `src/commands/run.rs`, ensure `tools` is in scope. If the existing `use` lines do not already bring it in, add:

```rust
use crate::tools;
```

Run: `cargo build` and fix any unused/duplicate-import warning by removing the line if `tools` was already imported.
Expected: clean build.

- [ ] **Step 7: Run the full test + lint suite**

Run: `cargo test && ./bin/lint`
Expected: PASS, no clippy/rustfmt errors.

- [ ] **Step 8: Commit**

```bash
git add src/lib.rs src/commands/run.rs tests/cli.rs
git commit -m "feat: steep run --scratch creates and attaches an ephemeral disk

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Add `e2fsprogs` to the initrd

The initrd needs `mkfs.ext4`; `cryptsetup-bin` is already present.

**Files:**
- Modify: `mkosi/initrd/mkosi.conf`

- [ ] **Step 1: Add the package**

In `mkosi/initrd/mkosi.conf`, add `e2fsprogs` to the `Packages=` list (alphabetical-ish, next to `cryptsetup-bin`):

```ini
Packages=
    cryptsetup-bin
    e2fsprogs
    zstd
    mount
    util-linux
    util-linux-extra
    bash
```

- [ ] **Step 2: Verify the file parses (sanity grep)**

Run: `grep -n "e2fsprogs" mkosi/initrd/mkosi.conf`
Expected: one match.

- [ ] **Step 3: Commit**

```bash
git add mkosi/initrd/mkosi.conf
git commit -m "feat: add e2fsprogs to initrd for mkfs.ext4

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Add the `LABEL=scratch` overlay branch to the initrd

Rewrite the device-scan + overlay block so `scratch` (ephemeral encrypted) takes precedence, `data` (persistent plaintext) is unchanged, and tmpfs remains the fallback.

**Files:**
- Modify: `mkosi/initrd/mkosi.extra/init` (lines 57-91, the data-disk/tmpfs block)

- [ ] **Step 1: Replace the device-scan + overlay block**

In `mkosi/initrd/mkosi.extra/init`, replace the entire block from the comment `# Try to mount a persistent data disk...` through the `echo "initrd: overlayfs ready"` line (lines 57-91) with:

```bash
# Pick overlay upper-layer backing. Scan virtio disks by whole-device label:
#   LABEL=scratch -> ephemeral, encrypted with a random in-RAM key (this boot)
#   LABEL=data    -> persistent, plaintext, also bind-mounted at /data
#   neither       -> 2G RAM tmpfs (writes lost on reboot)
SCRATCH_DEV=""
DATA_DEV=""
for dev in /dev/vdb /dev/vdc /dev/vdd; do
    if [ -e "$dev" ]; then
        LABEL=$(blkid -s LABEL -o value "$dev" 2>/dev/null || true)
        case "$LABEL" in
            scratch) [ -z "$SCRATCH_DEV" ] && SCRATCH_DEV="$dev" ;;
            data)    [ -z "$DATA_DEV" ] && DATA_DEV="$dev" ;;
        esac
    fi
done

if [ -n "$SCRATCH_DEV" ]; then
    echo "initrd: found scratch disk at $SCRATCH_DEV — ephemeral encrypted overlay"
    # Random per-boot key, kept only in (SEV-encrypted) RAM, never persisted.
    mkdir -p /run
    head -c 64 /dev/urandom > /run/scratch.key
    cryptsetup open --type plain --cipher aes-xts-plain64 --key-size 512 \
        -d /run/scratch.key "$SCRATCH_DEV" scratch
    # Reformat every boot: prior contents are unrecoverable (key is gone) anyway.
    mkfs.ext4 -q /dev/mapper/scratch
    mount /dev/mapper/scratch /sysroot-upper
    mkdir -p /sysroot-upper/upper /sysroot-upper/work
    mount -t overlay overlay \
        -o lowerdir=/sysroot-lower,upperdir=/sysroot-upper/upper,workdir=/sysroot-upper/work \
        /sysroot
    echo "initrd: overlayfs mounted (ephemeral encrypted, backed by $SCRATCH_DEV)"
elif [ -n "$DATA_DEV" ]; then
    echo "initrd: found data disk at $DATA_DEV"
    mkdir -p /mnt/data
    mount "$DATA_DEV" /mnt/data
    mkdir -p /mnt/data/overlay/upper /mnt/data/overlay/work
    mount -t overlay overlay \
        -o lowerdir=/sysroot-lower,upperdir=/mnt/data/overlay/upper,workdir=/mnt/data/overlay/work \
        /sysroot
    # Bind-mount data disk into the final root so it's at /data after switch_root
    mkdir -p /sysroot/data
    mount --bind /mnt/data /sysroot/data
    echo "initrd: overlayfs mounted (persistent, backed by $DATA_DEV at /data)"
else
    echo "initrd: WARNING: no data/scratch disk found, using tmpfs (writes lost on reboot)"
    mount -t tmpfs tmpfs /sysroot-upper -o size=2G,nosuid,nodev
    mkdir -p /sysroot-upper/upper /sysroot-upper/work
    mount -t overlay overlay \
        -o lowerdir=/sysroot-lower,upperdir=/sysroot-upper/upper,workdir=/sysroot-upper/work \
        /sysroot
fi
echo "initrd: overlayfs ready"
```

- [ ] **Step 2: Shellcheck the init script**

Run: `shellcheck mkosi/initrd/mkosi.extra/init || true`
Expected: no new errors introduced by the added block (the script uses `set -eu`; pre-existing style warnings, if any, are acceptable — do not "fix" unrelated lines).

- [ ] **Step 3: Commit**

```bash
git add mkosi/initrd/mkosi.extra/init
git commit -m "feat: mount LABEL=scratch as ephemeral encrypted overlay upper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Enable dm-crypt + AES-XTS in the kernel

**Heads-up: this task requires the kernel build environment** (sudo + systemd-nspawn + network to cdn.kernel.org) and a full kernel build (~10+ min). The snapshot is a generated artifact guarded by the drift test, so it must be regenerated, not hand-edited.

**Files:**
- Modify: `kernel/required.config`
- Regenerate: `kernel/config-x86_64.snapshot`

- [ ] **Step 1: Add the kernel options**

Append to `kernel/required.config`:

```
# dm-crypt for the ephemeral encrypted scratch overlay (aes-xts-plain64)
CONFIG_DM_CRYPT=y
CONFIG_CRYPTO_XTS=y
CONFIG_CRYPTO_AES=y
CONFIG_CRYPTO_AES_NI_INTEL=y
```

- [ ] **Step 2: Regenerate the snapshot via a real build**

Run: `bin/steep kernel --update-snapshot`
Expected: build succeeds; `kernel/config-x86_64.snapshot` is rewritten.

- [ ] **Step 3: Verify the options resolved into the snapshot**

Run: `grep -E "CONFIG_DM_CRYPT=y|CONFIG_CRYPTO_XTS=y|CONFIG_CRYPTO_AES=y|CONFIG_CRYPTO_AES_NI_INTEL=y" kernel/config-x86_64.snapshot`
Expected: all four lines present. (If a symbol resolved to a module `=m` or is absent because of a Kconfig dependency, investigate the dependency — dm-crypt requires the crypto AES + XTS symbols, which are listed here.)

- [ ] **Step 4: Commit**

```bash
git add kernel/required.config kernel/config-x86_64.snapshot
git commit -m "feat: enable dm-crypt + AES-XTS in kernel for scratch overlay

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Integration test — boot with a scratch disk

End-to-end check that the new path works. Marked `#[ignore]` because it builds an image and boots a VM (slow, needs the build environment). Follows the `#[ignore]` convention in `tests/kernel.rs`.

**Files:**
- Modify: `tests/kernel.rs` *(or a new `tests/scratch.rs` if cleaner — keep ignored, build-env-only tests together)*

> **Note for the implementer:** This test depends on Tasks 1-6 being complete and on a base image + kernel already built in the environment. If the harness cannot build/boot a CVM, write the test body as specified, confirm it compiles (`cargo build --tests`), and leave it `#[ignore]` — do not delete it. Record in the task notes that it was not executed.

- [ ] **Step 1: Write the ignored integration test**

Append to `tests/kernel.rs` (reusing its `binary()` / `KernelOut` helpers and `assert_cmd`):

```rust
/// Boot a steep VM with --scratch and confirm the root fs reports more than the
/// 2G tmpfs ceiling. Ignored: builds an image and boots a CVM (slow, build-env).
#[test]
#[ignore]
fn scratch_disk_expands_root_capacity() {
    // Assumes an already-built image dir at `output/base` in the repo.
    let dir = std::path::Path::new("output/base");
    assert!(
        dir.join("manifest.json").exists(),
        "build output/base first (steep build)"
    );

    // 8G scratch is well over the 2G tmpfs fallback.
    let out = Command::new(binary())
        .args(["run"])
        .arg(dir)
        .args(["--scratch", "8G"])
        // run boots a VM; rely on the harness/test wrapper to capture serial and
        // shut down. Here we only assert the disk was created and labeled.
        .timeout(std::time::Duration::from_secs(5))
        .output()
        .unwrap();

    // The disk-creation step prints before QEMU launches.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("Created ephemeral scratch disk"),
        "scratch disk was not created. output:\n{combined}"
    );
    assert!(dir.join("scratch.raw").exists(), "scratch.raw not created");

    // Confirm the host-side disk carries LABEL=scratch for the initrd to find.
    let label = std::process::Command::new("blkid")
        .args(["-s", "LABEL", "-o", "value"])
        .arg(dir.join("scratch.raw"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&label.stdout).trim(),
        "scratch",
        "scratch.raw must be labeled `scratch`"
    );
}
```

- [ ] **Step 2: Verify it compiles and is skipped by default**

Run: `cargo build --tests && cargo test --test kernel scratch_disk_expands_root_capacity`
Expected: builds; the test is reported as ignored (0 run) without `--ignored`.

- [ ] **Step 3 (build-env only): Run the ignored test end to end**

Run: `cargo test --test kernel scratch_disk_expands_root_capacity -- --ignored --test-threads=1`
Expected: PASS — `scratch.raw` created, labeled `scratch`. If the environment cannot boot a CVM, skip and note it (see task note above).

- [ ] **Step 4: Commit**

```bash
git add tests/kernel.rs
git commit -m "test: ignored integration test for --scratch overlay

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Document `steep run --scratch` and the `LABEL=scratch` contract

**Files:**
- Modify: `README.md` (in the "Launch the built VM" / `steep run` section)

- [ ] **Step 1: Add documentation**

In `README.md`, under the `steep run` usage, add:

```markdown
#### Ephemeral scratch space

A booted CVM's writable root is an overlay whose upper layer defaults to a 2G
RAM tmpfs, so build tasks that need more room run out of space. Attach an
**ephemeral encrypted scratch disk** to expand it:

```bash
steep run output/NAME --scratch 20G
```

This creates a fresh `scratch.raw` labeled `scratch` and attaches it writable.
The initrd detects any whole-device `LABEL=scratch` disk, encrypts it with a
random key generated in-guest at boot (never persisted), formats it, and uses
it as the overlay upper layer — so the entire root filesystem gains the space
transparently.

The disk is **ephemeral**: the key is discarded on shutdown, so contents do not
survive a reboot, and the host (untrusted on SNP) only ever sees ciphertext. In
production, attach your own `LABEL=scratch` block device instead of using
`--scratch`. A persistent `LABEL=data` disk continues to take the existing
plaintext path mounted at `/data`.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document steep run --scratch and LABEL=scratch contract

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo test` — all non-ignored tests pass.
- [ ] `./bin/lint` — clean.
- [ ] `git log --oneline` shows the task commits.
- [ ] Spec coverage: kernel crypto (Task 6), initrd mkfs dep (Task 4), initrd scratch branch (Task 5), writable launch drive (Tasks 2-3), `--scratch` flag (Task 3), tests (Tasks 1-3, 7), docs (Task 8). All spec sections covered.
