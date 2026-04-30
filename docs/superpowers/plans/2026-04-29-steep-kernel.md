# `steep kernel` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `steep kernel` subcommand that builds a hardened, reproducible Linux kernel from upstream source, and have `steep build` consume it as part of the existing image pipeline.

**Architecture:** A new mkosi tree (`mkosi/kernel-builder/`) provides a hermetic toolchain. `steep kernel` fetches a SHA-pinned kernel tarball from kernel.org, applies a fragment-merged config, builds inside `systemd-nspawn`, and writes `output/kernel/vmlinuz` + a fingerprinted manifest. `steep build` calls a shared `kernel_cache::ensure_kernel` that hits the cache or rebuilds on input drift, pre-stages the kernel into `mkosi/base/mkosi.extra/usr/lib/modules/<ver>/vmlinuz` (RAII cleanup), and proceeds through the existing initrd → base → IGVM phases. Coordinated changes drop `linux-generic`, drop runtime modules from kernel + initrd, and switch the console from 8250 (`ttyS0`) to virtio-console (`hvc0`).

**Tech Stack:** Rust (anyhow, serde, sha2, fs-err, hex, clap), mkosi v26, systemd-nspawn, GNU Make, kernel.org tarballs, QEMU.

**Spec:** `docs/superpowers/specs/2026-04-29-steep-kernel-design.md`

---

## File Structure

**New files:**

| Path | Responsibility |
|---|---|
| `kernel/version` | Pin kernel version + tarball SHA256. KEY=VALUE format. |
| `kernel/required.config` | Functional kconfig fragment (filesystems, dm-verity, SEV-SNP guest). |
| `kernel/hardening.config` | Security-posture kconfig fragment (operator-supplied). |
| `kernel/config-x86_64.snapshot` | Full resolved `.config`, regenerated only by `steep kernel --update-snapshot`. |
| `mkosi/kernel-builder/mkosi.conf` | Toolchain image (gcc, binutils, flex, bison, …) pinned to existing apt snapshot. |
| `src/kernel/mod.rs` | Module aggregator: `pub mod version; pub mod fetch; pub mod config; pub mod compile; pub mod manifest;`. |
| `src/kernel/version.rs` | Parse `kernel/version`. Pure logic. |
| `src/kernel/fetch.rs` | Download + SHA-verify tarball. |
| `src/kernel/config.rs` | Configure phase: defconfig + merge + mod2yesconfig + olddefconfig + snapshot guard. |
| `src/kernel/compile.rs` | Compile phase: nspawn `make bzImage` with reproducibility env, tee log. |
| `src/kernel/manifest.rs` | Read/write `output/kernel/manifest.json`; fingerprint canonical-JSON serialization. |
| `src/kernel_cache.rs` | `compute_fingerprint`, `ensure_kernel`. Used by `commands::build` and `commands::kernel`. |
| `tests/kernel.rs` | Integration tests (`#[ignore]`-gated; reproducibility, cache, drift). |
| `tests/fixtures/kernel_manifest.json` | Golden manifest fixture for unit tests. |

**Modified files:**

| Path | Change |
|---|---|
| `Cargo.toml` | Add no new deps (uses `which`, `anyhow`, `serde`, `serde_json`, `sha2`, `hex`, `fs-err`, `tracing` — all present). |
| `src/lib.rs` | Reshape `KernelArgs`; add `pub mod kernel; pub mod kernel_cache;`. |
| `src/main.rs` | No change (dispatch already routes Kernel → commands::kernel::run). |
| `src/commands/kernel.rs` | Replace TODO stub with full implementation orchestrating `src/kernel/*`. |
| `src/commands/build.rs` | New Phase 1 calling `kernel_cache::ensure_kernel`, RAII pre-staging, renumbered step banners (N/4); change `--console` autologin path from `ttyS0` to `hvc0`. |
| `src/manifest.rs` | Extend `ManifestInputs` with optional `kernel: Option<KernelInputs>`; add `KernelInputs` struct. |
| `src/qemu.rs` | Replace `-serial mon:stdio` with virtio-console wiring across all three tiers. |
| `mkosi/base/mkosi.conf` | Drop `linux-generic`, drop `KernelModulesInitrd*`, change cmdline `console=ttyS0 earlyprintk=serial` → `console=hvc0`. |
| `mkosi/initrd/mkosi.conf` | Drop `kmod` from `Packages=`. |
| `mkosi/initrd/mkosi.extra/init` | Delete `depmod -a` + `modprobe` loop (current lines 9–18). |
| `tests/qemu.rs` | Add virtio-console assertions; remove ttyS0-related ones. |
| `tests/e2e.sh` | Switch serial-log scraping from ttyS0 to hvc0; cloud-init `exec > /dev/ttyS0` → `/dev/hvc0`. |
| `.gitignore` | Add `/output/kernel/` and `/mkosi/base/mkosi.extra/usr/lib/modules/`. |

---

## Conventions used in this plan

- **Commit messages**: imperative, lowercase, no conventional-commit prefix (matching existing repo style: `add pull command`, `update tests`, `rename publish to push`).
- **Test runner**: `cargo test` for unit tests, `cargo test -- --ignored` for integration tests that need network or nspawn.
- **All paths** are relative to repo root `/home/andre/steep`.
- **Branch**: `kernel-subcommand` (already created).

---

# Phase A — Bootstrap files

## Task A1: Add `.gitignore` entries

**Files:**
- Modify: `.gitignore`

- [ ] **Step 1: Inspect current `.gitignore`**

```bash
cat .gitignore
```

- [ ] **Step 2: Append kernel-related ignores**

Append to `.gitignore` (do not duplicate any line that already exists):

```
/output/kernel/
/mkosi/kernel-builder/mkosi.output/
/mkosi/base/mkosi.extra/usr/lib/modules/
```

- [ ] **Step 3: Verify**

```bash
git check-ignore -v output/kernel/foo mkosi/kernel-builder/mkosi.output/x mkosi/base/mkosi.extra/usr/lib/modules/y
```

Expected: each path prints with a matching ignore rule.

- [ ] **Step 4: Commit**

```bash
git add .gitignore
git commit -m "ignore kernel build outputs and staged kernel"
```

---

## Task A2: Create `kernel/required.config`

**Files:**
- Create: `kernel/required.config`

- [ ] **Step 1: Write the file**

```
# Filesystems we mount inside the steep image
CONFIG_EROFS_FS=y
CONFIG_OVERLAY_FS=y
CONFIG_EXT4_FS=y
CONFIG_TMPFS=y

# Device mapper + dm-verity (verified root)
CONFIG_BLK_DEV_DM=y
CONFIG_DM_VERITY=y
CONFIG_DM_BUFIO=y

# AMD SEV-SNP guest
CONFIG_AMD_MEM_ENCRYPT=y
CONFIG_X86_MEM_ENCRYPT=y
CONFIG_AMD_MEM_ENCRYPT_ACTIVE_BY_DEFAULT=y

# devtmpfs auto-mounted by kernel (initrd uses /dev)
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
```

- [ ] **Step 2: Commit**

```bash
git add kernel/required.config
git commit -m "add kernel/required.config functional fragment"
```

---

## Task A3: Create `kernel/hardening.config`

**Files:**
- Create: `kernel/hardening.config`

- [ ] **Step 1: Write the file**

This is the operator-supplied hardening posture, deduped from the originally pasted list:

```
# Minimal virtio set (everything else off)
CONFIG_VIRTIO=y
CONFIG_VIRTIO_PCI=y
CONFIG_VIRTIO_BLK=y
CONFIG_VIRTIO_NET=y
CONFIG_VIRTIO_CONSOLE=y
CONFIG_VIRTIO_VSOCKETS=y

# Disable large attack surfaces
# CONFIG_USB_SUPPORT is not set
# CONFIG_SOUND is not set
# CONFIG_DRM is not set
# CONFIG_WIRELESS is not set
# CONFIG_BLUETOOTH is not set
# CONFIG_INPUT_MISC is not set
# CONFIG_HW_RANDOM_VIRTIO is not set
# CONFIG_NET_9P is not set

# PCI hotplug + paravirt features (host-controllable; off in confidential VM)
# CONFIG_HOTPLUG_PCI is not set
# CONFIG_HOTPLUG_PCI_ACPI is not set
# CONFIG_HOTPLUG_PCI_PCIE is not set
# CONFIG_PCI_QUIRKS is not set
# CONFIG_PCI_ATS is not set
# CONFIG_PCI_IOV is not set
# CONFIG_PCI_PRI is not set
# CONFIG_PCI_PASID is not set
# CONFIG_PCI_LABEL is not set
# CONFIG_IOMMU_SVA is not set

# RDRAND-trusted RNG (no virtio-rng)
CONFIG_ARCH_RANDOM=y
CONFIG_RANDOM_TRUST_CPU=y

# Lockdown LSM in confidentiality mode (early)
CONFIG_SECURITY_LOCKDOWN_LSM=y
CONFIG_SECURITY_LOCKDOWN_LSM_EARLY=y
CONFIG_LOCK_DOWN_KERNEL_FORCE_CONFIDENTIALITY=y

# Memory-safety hardening
CONFIG_STACKPROTECTOR=y
CONFIG_STACKPROTECTOR_STRONG=y
CONFIG_HARDENED_USERCOPY=y
CONFIG_SLAB_FREELIST_RANDOM=y
CONFIG_SLAB_FREELIST_HARDENED=y

# ACPI minimization
# CONFIG_ACPI_DOCK is not set
# CONFIG_ACPI_PROCESSOR is not set
# CONFIG_ACPI_HOTPLUG_CPU is not set
# CONFIG_ACPI_HOTPLUG_MEMORY is not set
# CONFIG_ACPI_CUSTOM_DSDT is not set
# CONFIG_ACPI_DEBUG is not set
# CONFIG_ACPI_PCI_SLOT is not set
# CONFIG_ACPI_BGRT is not set
# CONFIG_ACPI_TABLE_UPGRADE is not set

# Port-IO surface reduction (legacy devices off; switching console to hvc0)
# CONFIG_SERIAL_8250 is not set
# CONFIG_KEYBOARD_ATKBD is not set
# CONFIG_SERIO_I8042 is not set
# CONFIG_RTC_DRV_CMOS is not set
# CONFIG_PCSPKR_PLATFORM is not set

# No runtime modules; everything built-in (mod2yesconfig is defense-in-depth)
# CONFIG_MODULES is not set

# No debug info (smaller artifacts; one less reproducibility surface)
# CONFIG_DEBUG_INFO is not set
# CONFIG_LOCALVERSION_AUTO is not set
```

- [ ] **Step 2: Commit**

```bash
git add kernel/hardening.config
git commit -m "add kernel/hardening.config security fragment"
```

---

## Task A4: Create `kernel/version` placeholder

We pick a specific upstream release at the top of Phase D once we know everything compiles. For now, write a placeholder with the latest 6.12 LTS at time of writing. The exact version is verified by Task D7's first build.

**Files:**
- Create: `kernel/version`

- [ ] **Step 1: Look up latest stable 6.12.x and its tarball SHA**

Visit `https://www.kernel.org/`. Read the latest 6.12.x version (a maintained LTS line). Fetch the SHA256 from `https://cdn.kernel.org/pub/linux/kernel/v6.x/sha256sums.asc` (or compute it after download in Task D7).

For now, populate with the version number known at planning time and a placeholder SHA that we will fix in Task D7. Mark the SHA explicitly as TBD-but-must-be-set-before-merge.

- [ ] **Step 2: Write the file**

Replace `<sha-from-step-1>` with the SHA from kernel.org's signed sums file:

```
LINUX_VERSION=6.12.7
LINUX_TARBALL_SHA256=<sha-from-step-1>
```

- [ ] **Step 3: Commit**

```bash
git add kernel/version
git commit -m "pin kernel version 6.12.7 + tarball sha"
```

---

## Task A5: Create `mkosi/kernel-builder/mkosi.conf`

**Files:**
- Create: `mkosi/kernel-builder/mkosi.conf`

- [ ] **Step 1: Write the file**

```
[Config]
MinimumVersion=26

[Distribution]
Architecture=x86-64
Distribution=ubuntu
Release=resolute
Repositories=universe
Mirror=https://snapshot.ubuntu.com/ubuntu/20260405T000000Z

[Build]
Incremental=false
ToolsTree=default

[Content]
Bootable=false
SourceDateEpoch=0
CleanPackageMetadata=yes
WithDocs=false
Packages=
    bash
    bc
    binutils
    bison
    coreutils
    cpio
    curl
    findutils
    flex
    gcc
    grep
    gzip
    libelf-dev
    libssl-dev
    make
    perl
    rsync
    sed
    tar
    util-linux
    xz-utils

[Output]
Format=directory
Seed=d4f09d27-7e4e-4b1a-9c3a-deadbeef0003
```

- [ ] **Step 2: Verify mkosi parses it**

```bash
mkosi --directory mkosi/kernel-builder summary
```

Expected: prints a summary without errors. (Does NOT build yet.)

- [ ] **Step 3: Commit**

```bash
git add mkosi/kernel-builder/mkosi.conf
git commit -m "add mkosi/kernel-builder toolchain tree config"
```

---

# Phase B — Pure-logic Rust modules (TDD)

All tasks in Phase B are pure logic — no filesystem, no network, no subprocess. Unit-tested with `cargo test`.

## Task B1: `src/kernel/mod.rs` and `src/kernel/version.rs` skeleton

**Files:**
- Create: `src/kernel/mod.rs`
- Create: `src/kernel/version.rs`
- Modify: `src/lib.rs:1-4` (the `pub mod` block at the top)

- [ ] **Step 1: Add `pub mod kernel;` to `src/lib.rs`**

In `src/lib.rs`, the top of the file currently reads:

```rust
pub mod igvm;
pub mod manifest;
pub mod qemu;
pub mod tools;
```

Change to:

```rust
pub mod igvm;
pub mod kernel;
pub mod kernel_cache;
pub mod manifest;
pub mod qemu;
pub mod tools;
```

- [ ] **Step 2: Create `src/kernel/mod.rs`**

```rust
pub mod compile;
pub mod config;
pub mod fetch;
pub mod manifest;
pub mod version;
```

- [ ] **Step 3: Create `src/kernel/version.rs` skeleton**

```rust
//! Parse the `kernel/version` pin file.

use anyhow::{anyhow, Context, Result};

/// Parsed contents of `kernel/version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelVersion {
    pub linux_version: String,
    pub tarball_sha256: String,
}

impl KernelVersion {
    /// Parse a KEY=VALUE blob (one per line; `#` comments and blank lines OK).
    pub fn parse(s: &str) -> Result<Self> {
        // Implemented in B2.
        let _ = s;
        Err(anyhow!("not yet implemented"))
    }
}
```

- [ ] **Step 4: Create `src/kernel_cache.rs` skeleton**

For now just a placeholder — the real module is Task D5.

```rust
//! Cache lookup + fingerprinting for the custom kernel build.
//!
//! Real implementation in Task D5.
```

- [ ] **Step 5: Verify the crate still builds**

```bash
cargo build
```

Expected: builds clean (with one or two `dead_code` warnings — fine).

- [ ] **Step 6: Commit**

```bash
git add src/lib.rs src/kernel/mod.rs src/kernel/version.rs src/kernel_cache.rs
git commit -m "scaffold src/kernel and src/kernel_cache modules"
```

---

## Task B2: `KernelVersion::parse` (TDD)

**Files:**
- Modify: `src/kernel/version.rs`

- [ ] **Step 1: Write the failing tests**

Append to the bottom of `src/kernel/version.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_valid_input() {
        let v = KernelVersion::parse(
            "LINUX_VERSION=6.12.7\nLINUX_TARBALL_SHA256=abc123\n",
        )
        .unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc123");
    }

    #[test]
    fn ignores_blank_lines_and_comments() {
        let v = KernelVersion::parse(
            "# pinned version\n\nLINUX_VERSION=6.12.7\n# tarball hash\nLINUX_TARBALL_SHA256=abc\n",
        )
        .unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc");
    }

    #[test]
    fn fails_when_linux_version_missing() {
        let err = KernelVersion::parse("LINUX_TARBALL_SHA256=abc\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_VERSION"));
    }

    #[test]
    fn fails_when_sha_missing() {
        let err = KernelVersion::parse("LINUX_VERSION=6.12.7\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_TARBALL_SHA256"));
    }

    #[test]
    fn fails_on_unknown_key() {
        let err = KernelVersion::parse("LINUX_VERSION=1\nLINUX_TARBALL_SHA256=a\nBOGUS=x\n")
            .unwrap_err();
        assert!(err.to_string().contains("BOGUS"));
    }

    #[test]
    fn trims_whitespace_around_value() {
        let v = KernelVersion::parse("LINUX_VERSION=  6.12.7  \nLINUX_TARBALL_SHA256=  abc  \n")
            .unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc");
    }

    #[test]
    fn fails_on_malformed_line() {
        let err = KernelVersion::parse("LINUX_VERSION 6.12.7\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_VERSION"));
    }
}
```

- [ ] **Step 2: Run tests; expect failures**

```bash
cargo test --lib kernel::version
```

Expected: all tests fail with `not yet implemented`.

- [ ] **Step 3: Implement `parse`**

Replace the `parse` body in `src/kernel/version.rs`:

```rust
impl KernelVersion {
    pub fn parse(s: &str) -> Result<Self> {
        let mut linux_version: Option<String> = None;
        let mut tarball_sha256: Option<String> = None;

        for (lineno, raw) in s.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| anyhow!("kernel/version: line {} malformed: {}", lineno + 1, raw))?;
            let value = v.trim().to_string();
            match k.trim() {
                "LINUX_VERSION" => linux_version = Some(value),
                "LINUX_TARBALL_SHA256" => tarball_sha256 = Some(value),
                other => return Err(anyhow!("kernel/version: unknown key '{}'", other)),
            }
        }

        Ok(KernelVersion {
            linux_version: linux_version
                .ok_or_else(|| anyhow!("kernel/version: missing LINUX_VERSION"))?,
            tarball_sha256: tarball_sha256
                .ok_or_else(|| anyhow!("kernel/version: missing LINUX_TARBALL_SHA256"))?,
        })
    }

    /// Read and parse `kernel/version` from disk.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let content = fs_err::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&content)
    }
}
```

- [ ] **Step 4: Re-run tests**

```bash
cargo test --lib kernel::version
```

Expected: 7/7 pass.

- [ ] **Step 5: Commit**

```bash
git add src/kernel/version.rs
git commit -m "implement kernel/version parser with tests"
```

---

## Task B3: `src/kernel/manifest.rs` — fingerprint + manifest types (TDD)

**Files:**
- Create: `src/kernel/manifest.rs` (overwriting the empty file from B1; B1 declared it via `pub mod manifest;`).
- Create: `tests/fixtures/kernel_manifest.json`

- [ ] **Step 1: Decide the on-disk shape**

The fingerprint object MUST be canonical-JSON: keys sorted, no whitespace, no trailing commas. We use `BTreeMap<&str, String>` to get sorted-key serialization for free.

- [ ] **Step 2: Write the module skeleton**

```rust
//! `output/kernel/manifest.json` schema and fingerprint helpers.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KernelManifest {
    pub version: u32,
    pub linux_version: String,
    pub inputs: Fingerprint,
    pub outputs: Outputs,
    pub built_at: String,
}

/// Canonical fingerprint of all inputs that determine the kernel build output.
/// Field order here MUST match the BTreeMap iteration in `to_canonical_json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub linux_version: String,
    pub tarball_sha256: String,
    pub required_config_sha256: String,
    pub hardening_config_sha256: String,
    pub snapshot_config_sha256: String,
    pub tools_tree_digest: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Outputs {
    pub vmlinuz_sha256: String,
}

impl Fingerprint {
    /// Render this fingerprint as canonical JSON: keys sorted, no whitespace.
    /// Used to compare fingerprints across runs.
    pub fn to_canonical_json(&self) -> String {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        m.insert("linux_version", &self.linux_version);
        m.insert("tarball_sha256", &self.tarball_sha256);
        m.insert("required_config_sha256", &self.required_config_sha256);
        m.insert("hardening_config_sha256", &self.hardening_config_sha256);
        m.insert("snapshot_config_sha256", &self.snapshot_config_sha256);
        m.insert("tools_tree_digest", &self.tools_tree_digest);
        serde_json::to_string(&m).expect("BTreeMap of strings serializes")
    }
}

pub fn read(path: &Path) -> Result<KernelManifest> {
    let s = fs_err::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let m: KernelManifest = serde_json::from_str(&s)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(m)
}

pub fn write(path: &Path, manifest: &KernelManifest) -> Result<()> {
    let s = serde_json::to_string_pretty(manifest)?;
    fs_err::write(path, s).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 3: Create the golden fixture**

Create `tests/fixtures/kernel_manifest.json` with a deterministic example:

```json
{
  "version": 1,
  "linux_version": "6.12.7",
  "inputs": {
    "linux_version": "6.12.7",
    "tarball_sha256": "0000000000000000000000000000000000000000000000000000000000000001",
    "required_config_sha256": "0000000000000000000000000000000000000000000000000000000000000002",
    "hardening_config_sha256": "0000000000000000000000000000000000000000000000000000000000000003",
    "snapshot_config_sha256": "0000000000000000000000000000000000000000000000000000000000000004",
    "tools_tree_digest": "0000000000000000000000000000000000000000000000000000000000000005"
  },
  "outputs": {
    "vmlinuz_sha256": "0000000000000000000000000000000000000000000000000000000000000006"
  },
  "built_at": "2026-04-29T12:00:00Z"
}
```

- [ ] **Step 4: Write the failing tests**

Append to `src/kernel/manifest.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fp() -> Fingerprint {
        Fingerprint {
            linux_version: "6.12.7".into(),
            tarball_sha256: "a".repeat(64),
            required_config_sha256: "b".repeat(64),
            hardening_config_sha256: "c".repeat(64),
            snapshot_config_sha256: "d".repeat(64),
            tools_tree_digest: "e".repeat(64),
        }
    }

    #[test]
    fn fingerprint_canonical_json_keys_sorted() {
        let json = sample_fp().to_canonical_json();
        // BTreeMap renders alphabetically: hardening, linux, required, snapshot, tarball, tools.
        let h = json.find("hardening_config_sha256").unwrap();
        let l = json.find("linux_version").unwrap();
        let r = json.find("required_config_sha256").unwrap();
        let s = json.find("snapshot_config_sha256").unwrap();
        let t = json.find("tarball_sha256").unwrap();
        let tt = json.find("tools_tree_digest").unwrap();
        assert!(h < l && l < r && r < s && s < t && t < tt);
    }

    #[test]
    fn fingerprint_canonical_json_no_whitespace() {
        let json = sample_fp().to_canonical_json();
        assert!(!json.contains(' '));
        assert!(!json.contains('\n'));
    }

    #[test]
    fn fingerprint_round_trips_via_serde() {
        let fp = sample_fp();
        let s = serde_json::to_string(&fp).unwrap();
        let back: Fingerprint = serde_json::from_str(&s).unwrap();
        assert_eq!(fp, back);
    }

    #[test]
    fn equal_fingerprints_render_equal_json() {
        let a = sample_fp();
        let b = sample_fp();
        assert_eq!(a.to_canonical_json(), b.to_canonical_json());
    }

    #[test]
    fn changing_one_field_changes_json() {
        let mut a = sample_fp();
        let b = a.clone();
        a.linux_version = "6.12.8".into();
        assert_ne!(a.to_canonical_json(), b.to_canonical_json());
    }

    #[test]
    fn read_parses_golden_fixture() {
        let path = std::path::Path::new("tests/fixtures/kernel_manifest.json");
        let m = read(path).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.linux_version, "6.12.7");
        assert_eq!(m.outputs.vmlinuz_sha256.len(), 64);
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test --lib kernel::manifest
```

Expected: 6/6 pass (manifest.rs is already complete from Step 2).

- [ ] **Step 6: Commit**

```bash
git add src/kernel/manifest.rs tests/fixtures/kernel_manifest.json
git commit -m "implement kernel manifest + fingerprint with tests"
```

---

## Task B4: `src/kernel/config.rs` — snapshot diff guard (TDD)

This task implements only the diff/guard logic, *not* the kbuild orchestration. The kbuild glue lives in Task C2.

**Files:**
- Modify: `src/kernel/config.rs`

- [ ] **Step 1: Write skeleton + failing tests**

Replace the empty `src/kernel/config.rs` with:

```rust
//! Kernel `.config` resolution and snapshot guard.
//!
//! The "configure phase" runs `make x86_64_defconfig`, applies fragments,
//! then `mod2yesconfig`, then `olddefconfig`. After that, the resolved
//! `.config` is compared against the committed snapshot via [`check_snapshot`].

use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// Read two files and assert byte-equality. Returns Ok(()) on match.
/// On mismatch, error includes both paths so the caller can suggest a diff.
pub fn check_snapshot(resolved: &Path, snapshot: &Path) -> Result<()> {
    let a = fs_err::read(resolved)
        .with_context(|| format!("reading resolved config {}", resolved.display()))?;
    let b = fs_err::read(snapshot)
        .with_context(|| format!("reading snapshot {}", snapshot.display()))?;
    if a == b {
        Ok(())
    } else {
        Err(anyhow!(
            "kernel .config drift: {} differs from {}.\n\
             Review the diff and re-run with `steep kernel --update-snapshot` if intended.",
            resolved.display(),
            snapshot.display()
        ))
    }
}

/// Replace `snapshot` with the contents of `resolved`. Used by --update-snapshot.
pub fn update_snapshot(resolved: &Path, snapshot: &Path) -> Result<()> {
    let bytes = fs_err::read(resolved)
        .with_context(|| format!("reading resolved config {}", resolved.display()))?;
    fs_err::write(snapshot, bytes)
        .with_context(|| format!("writing snapshot {}", snapshot.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        fs_err::write(&p, content).unwrap();
        p
    }

    #[test]
    fn check_snapshot_passes_on_match() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=y\n");
        assert!(check_snapshot(&a, &b).is_ok());
    }

    #[test]
    fn check_snapshot_fails_on_diff_with_helpful_message() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=n\n");
        let err = check_snapshot(&a, &b).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(".config drift"));
        assert!(msg.contains("--update-snapshot"));
    }

    #[test]
    fn update_snapshot_overwrites_target() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "CONFIG_X=y\n");
        let b = write(&d, "b", "CONFIG_X=n\n");
        update_snapshot(&a, &b).unwrap();
        assert_eq!(fs_err::read_to_string(&b).unwrap(), "CONFIG_X=y\n");
    }

    #[test]
    fn check_snapshot_errors_on_missing_file() {
        let d = TempDir::new().unwrap();
        let a = write(&d, "a", "x");
        let b = d.path().join("does-not-exist");
        assert!(check_snapshot(&a, &b).is_err());
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test --lib kernel::config
```

Expected: 4/4 pass.

- [ ] **Step 3: Commit**

```bash
git add src/kernel/config.rs
git commit -m "implement snapshot guard with tests"
```

---

# Phase C — I/O glue

These tasks touch the filesystem, network, and subprocesses. Most logic still has unit tests; full end-to-end happens in Phase D.

## Task C1: `src/kernel/fetch.rs` — tarball download + SHA verify

**Files:**
- Modify: `src/kernel/fetch.rs`

- [ ] **Step 1: Write skeleton + tests**

Replace the empty `src/kernel/fetch.rs` with:

```rust
//! Fetch a kernel tarball from cdn.kernel.org and verify its SHA256.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::tools;

/// Download `linux-<version>.tar.xz` to `cache_dir`, verify SHA256.
/// Returns the path to the verified tarball.
/// Skips download if the cached file already passes the SHA check.
pub fn fetch(version: &str, expected_sha256: &str, cache_dir: &Path) -> Result<PathBuf> {
    fs_err::create_dir_all(cache_dir)?;
    let major_dir = major_dir_for(version)?;
    let url = format!(
        "https://cdn.kernel.org/pub/linux/kernel/{}/linux-{}.tar.xz",
        major_dir, version
    );
    let dest = cache_dir.join(format!("linux-{}.tar.xz", version));

    if dest.exists() {
        let actual = sha256_file(&dest)?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            tracing::info!(path = %dest.display(), "tarball cache hit");
            return Ok(dest);
        }
        tracing::warn!(path = %dest.display(), "cached tarball sha mismatch, re-downloading");
    }

    tracing::info!(%url, "fetching kernel tarball");
    tools::run_command_streaming(
        "curl",
        &[
            "--fail",
            "--show-error",
            "--silent",
            "--location",
            "--output",
            &dest.to_string_lossy(),
            &url,
        ],
    )
    .with_context(|| format!("downloading {}", url))?;

    let actual = sha256_file(&dest)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "tarball SHA256 mismatch for {}:\n  expected {}\n  actual   {}",
            dest.display(),
            expected_sha256,
            actual
        ));
    }
    Ok(dest)
}

/// Compute SHA256 of a file as a lowercase hex string.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs_err::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// Map a kernel semver to the cdn.kernel.org subdirectory.
/// Versions 6.x.y → "v6.x"; versions 5.x.y → "v5.x"; etc.
fn major_dir_for(version: &str) -> Result<String> {
    let major = version
        .split('.')
        .next()
        .ok_or_else(|| anyhow!("malformed version: {}", version))?;
    Ok(format!("v{}.x", major))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_dir_for_v6() {
        assert_eq!(major_dir_for("6.12.7").unwrap(), "v6.x");
    }

    #[test]
    fn major_dir_for_v5() {
        assert_eq!(major_dir_for("5.15.0").unwrap(), "v5.x");
    }

    #[test]
    fn sha256_file_known_value() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("hello");
        fs_err::write(&p, b"hello").unwrap();
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_file(&p).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
```

- [ ] **Step 2: Run unit tests**

```bash
cargo test --lib kernel::fetch
```

Expected: 3/3 pass.

- [ ] **Step 3: Commit**

```bash
git add src/kernel/fetch.rs
git commit -m "implement kernel tarball fetcher with sha verification"
```

---

## Task C2: `src/kernel/config.rs` — kbuild orchestration

Extends Task B4's snapshot module with the actual `make defconfig + merge + mod2yesconfig + olddefconfig` orchestration. Runs inside `systemd-nspawn` against the kernel-builder tools tree.

**Files:**
- Modify: `src/kernel/config.rs`

- [ ] **Step 1: Append the orchestrator function**

Append (above the `#[cfg(test)]` block) to `src/kernel/config.rs`:

```rust
use std::ffi::OsString;

/// Orchestrate the configure phase inside systemd-nspawn against the kernel-builder tools tree.
///
/// Inside the tools tree, runs (in this order):
///   make x86_64_defconfig
///   scripts/kconfig/merge_config.sh -m .config <required>
///   scripts/kconfig/merge_config.sh -m .config <hardening>
///   make mod2yesconfig
///   make olddefconfig
///
/// All paths are resolved on the host and bind-mounted into the nspawn at the same locations.
pub fn run_configure_phase(
    tools_tree: &Path,
    kernel_dir: &Path,
    required_fragment: &Path,
    hardening_fragment: &Path,
) -> Result<()> {
    let kernel_dir_abs = kernel_dir
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", kernel_dir.display()))?;
    let required_abs = required_fragment.canonicalize()?;
    let hardening_abs = hardening_fragment.canonicalize()?;

    // Inside nspawn, the kernel tree is mounted at /build, fragments at /build/.fragments/.
    // We bind-mount the fragments into the kernel dir so merge_config sees them.
    let frag_dir_in_kernel = kernel_dir_abs.join(".fragments");
    fs_err::create_dir_all(&frag_dir_in_kernel)?;
    fs_err::copy(&required_abs, frag_dir_in_kernel.join("required.config"))?;
    fs_err::copy(&hardening_abs, frag_dir_in_kernel.join("hardening.config"))?;

    let script = "\
set -eux
cd /build
make x86_64_defconfig
scripts/kconfig/merge_config.sh -m .config .fragments/required.config
scripts/kconfig/merge_config.sh -m .config .fragments/hardening.config
make mod2yesconfig
make olddefconfig
";

    nspawn(tools_tree, &kernel_dir_abs, "/build", &[("HOME", "/root")], script)?;
    fs_err::remove_dir_all(&frag_dir_in_kernel)?;
    Ok(())
}

/// Run a shell script inside `tools_tree` with `host_dir` bind-mounted at `mount_at`.
/// `env_vars` is `(name, value)` pairs forwarded via `--setenv`.
pub fn nspawn(
    tools_tree: &Path,
    host_dir: &Path,
    mount_at: &str,
    env_vars: &[(&str, &str)],
    script: &str,
) -> Result<()> {
    let nspawn = tools::require("systemd-nspawn")
        .map_err(|_| anyhow!("systemd-nspawn required; install systemd-container"))?;

    let mut args: Vec<OsString> = vec![
        OsString::from("--quiet"),
        OsString::from("--register=no"),
        OsString::from("--keep-unit"),
        OsString::from("--ephemeral"),
        OsString::from("--directory"),
        tools_tree.into(),
        OsString::from("--bind"),
        OsString::from(format!("{}:{}", host_dir.display(), mount_at)),
    ];
    for (k, v) in env_vars {
        args.push(OsString::from("--setenv"));
        args.push(OsString::from(format!("{}={}", k, v)));
    }
    args.push(OsString::from("/bin/bash"));
    args.push(OsString::from("-c"));
    args.push(OsString::from(script));

    let mut v: Vec<OsString> = vec![nspawn.as_os_str().into()];
    v.extend(args);
    tools::run_command_streaming("sudo", &v[..])
        .map_err(|e| anyhow!("nspawn failed: {}", e))
}
```

The `tools::run_command_streaming` signature requires `&[impl AsRef<OsStr>]`. `Vec<OsString>` satisfies that. If the borrow-checker complains about lifetimes, refactor to a helper `nspawn_inner(args: Vec<OsString>)` that owns its vec.

- [ ] **Step 2: Build (no new tests yet — this code path requires an actual tools tree to exercise; gated by Task D7 integration tests)**

```bash
cargo build
```

Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src/kernel/config.rs
git commit -m "add configure-phase nspawn orchestrator"
```

---

## Task C3: `src/kernel/compile.rs` — kbuild compile + tee log

**Files:**
- Modify: `src/kernel/compile.rs`

- [ ] **Step 1: Write the module**

Replace the empty `src/kernel/compile.rs` with:

```rust
//! Compile the kernel inside systemd-nspawn, with reproducibility env pinned.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Compile the kernel; copy the resulting bzImage to `out_vmlinuz`.
/// Tees nspawn stdout/stderr to `log_path`.
pub fn run(
    tools_tree: &Path,
    kernel_dir: &Path,
    out_vmlinuz: &Path,
    log_path: &Path,
) -> Result<()> {
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // Truncate the build log up front so it always reflects this run.
    fs_err::write(log_path, b"")?;

    let kernel_dir_abs = kernel_dir.canonicalize()?;
    let log_abs = log_path.canonicalize()?;

    let env = [
        ("SOURCE_DATE_EPOCH", "0"),
        ("KBUILD_BUILD_TIMESTAMP", "@0"),
        ("KBUILD_BUILD_USER", "steep"),
        ("KBUILD_BUILD_HOST", "steep"),
        ("KCONFIG_NOTIMESTAMP", "1"),
    ];

    let script = format!(
        "set -eux\n\
         cd /build\n\
         make -j{parallelism} bzImage 2>&1 | tee -a /build.log\n\
         test -f arch/x86/boot/bzImage\n",
        parallelism = parallelism,
    );

    // Bind-mount the log file in addition to the kernel dir.
    let host_dir = kernel_dir_abs.clone();
    let extra_bind = format!("{}:/build.log", log_abs.display());

    let nspawn = crate::tools::require("systemd-nspawn")
        .map_err(|_| anyhow!("systemd-nspawn required; install systemd-container"))?;

    let mut args: Vec<std::ffi::OsString> = vec![
        "--quiet".into(),
        "--register=no".into(),
        "--keep-unit".into(),
        "--ephemeral".into(),
        "--directory".into(),
        tools_tree.into(),
        "--bind".into(),
        format!("{}:/build", host_dir.display()).into(),
        "--bind".into(),
        extra_bind.into(),
    ];
    for (k, v) in &env {
        args.push("--setenv".into());
        args.push(format!("{}={}", k, v).into());
    }
    args.push("/bin/bash".into());
    args.push("-c".into());
    args.push(script.into());

    let mut full_args: Vec<std::ffi::OsString> = vec![nspawn.as_os_str().into()];
    full_args.extend(args);

    crate::tools::run_command_streaming("sudo", &full_args[..])
        .map_err(|e| anyhow!("kernel compile failed: {}. Full log: {}", e, log_path.display()))?;

    let bz = kernel_dir_abs.join("arch/x86/boot/bzImage");
    if !bz.exists() {
        return Err(anyhow!(
            "kernel build claimed success but bzImage missing at {}",
            bz.display()
        ));
    }
    fs_err::copy(&bz, out_vmlinuz)
        .with_context(|| format!("copying bzImage to {}", out_vmlinuz.display()))?;
    Ok(())
}

```

- [ ] **Step 2: Build**

```bash
cargo build
```

Expected: builds clean. (No tests yet — exercised by Task F1 integration test.)

- [ ] **Step 3: Commit**

```bash
git add src/kernel/compile.rs
git commit -m "add kernel compile phase with reproducibility env"
```

---

# Phase D — End-to-end wiring

## Task D1: Reshape `KernelArgs` in `src/lib.rs`

**Files:**
- Modify: `src/lib.rs:8-21` (the existing `KernelArgs` struct)
- Modify: `tests/cli.rs` (add a help-text assertion)

- [ ] **Step 1: Replace `KernelArgs`**

In `src/lib.rs`, find the existing `KernelArgs`:

```rust
#[derive(clap::Args)]
pub struct KernelArgs {
    /// Path to kernel source tree
    #[arg(long)]
    pub source: PathBuf,

    /// Path to kernel .config (hardening config)
    #[arg(long)]
    pub config: PathBuf,

    /// Output directory for kernel + initrd
    #[arg(short, long)]
    pub output: PathBuf,
}
```

Replace with:

```rust
#[derive(clap::Args)]
pub struct KernelArgs {
    /// Force rebuild even if cache is current
    #[arg(short, long)]
    pub force: bool,

    /// Regenerate kernel/config-x86_64.snapshot from defconfig + fragments.
    /// Use after bumping the kernel version or editing the fragments.
    #[arg(long)]
    pub update_snapshot: bool,

    /// Output directory.
    #[arg(short, long, default_value = "output/kernel")]
    pub output: PathBuf,
}
```

- [ ] **Step 2: Add a CLI help test**

Append to `tests/cli.rs`:

```rust
#[test]
fn test_kernel_help() {
    let mut cmd = Command::cargo_bin("steep").unwrap();
    cmd.args(["kernel", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("force"))
        .stdout(predicates::str::contains("update-snapshot"))
        .stdout(predicates::str::contains("output"));
}
```

- [ ] **Step 3: Build + run cli tests**

```bash
cargo build
cargo test --test cli test_kernel_help
```

Expected: builds; test passes. (The existing TODO stub in `commands::kernel::run` ignores the new fields — fine for now.)

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs tests/cli.rs
git commit -m "reshape KernelArgs CLI surface"
```

---

## Task D2: Implement `commands::kernel::run`

This task wires the Phase B/C modules together. Pulls fragment paths from convention.

**Files:**
- Modify: `src/commands/kernel.rs` (full rewrite)
- Modify: `src/kernel/compile.rs` (remove the `_ref_config` shim from C3)

- [ ] **Step 1: Replace `commands::kernel::run`**

```rust
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::kernel::{compile, config, fetch, manifest as km, version::KernelVersion};
use crate::tools;
use crate::KernelArgs;

const REQUIRED_FRAGMENT: &str = "kernel/required.config";
const HARDENING_FRAGMENT: &str = "kernel/hardening.config";
const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const VERSION_PATH: &str = "kernel/version";
const TOOLS_TREE_DIR: &str = "mkosi/kernel-builder";

pub fn run(args: &KernelArgs) -> Result<()> {
    let version = KernelVersion::read(Path::new(VERSION_PATH))?;
    tracing::info!(linux_version = %version.linux_version, "building hardened kernel");

    fs_err::create_dir_all(&args.output)?;
    let out_dir = args.output.canonicalize()?;
    let cache_dir = out_dir.join("cache");
    let build_dir = out_dir.join("build");
    let log_path = out_dir.join("build.log");
    let vmlinuz_path = out_dir.join("vmlinuz");
    let manifest_path = out_dir.join("manifest.json");

    // Cache short-circuit: skip the entire build if all inputs match and the
    // existing vmlinuz still hashes to what the manifest claims. --force and
    // --update-snapshot both bypass this.
    if !args.force && !args.update_snapshot && manifest_path.exists() && vmlinuz_path.exists() {
        if let Ok(cached) = km::read(&manifest_path) {
            let tools_tree_path = Path::new("mkosi/kernel-builder/mkosi.output/image");
            if let Ok(live) = compute_fingerprint(&version, tools_tree_path) {
                if cached.inputs == live {
                    let actual = fetch::sha256_file(&vmlinuz_path)?;
                    if actual.eq_ignore_ascii_case(&cached.outputs.vmlinuz_sha256) {
                        println!(
                            "kernel cache HIT (linux {}, sha256 {})",
                            cached.linux_version, actual
                        );
                        return Ok(());
                    }
                    return Err(anyhow!(
                        "kernel artifact corrupted (sha256 mismatch). Re-run with --force."
                    ));
                }
            }
        }
    }

    // Phase 0a: ensure tools tree
    println!("\n=== Step 0a: Ensuring kernel-builder tools tree (mkosi) ===");
    let tools_tree = ensure_tools_tree()?;

    // Phase 0b: fetch tarball
    println!("\n=== Step 0b: Fetching kernel tarball ===");
    let tarball = fetch::fetch(&version.linux_version, &version.tarball_sha256, &cache_dir)?;

    // Phase 0c: extract + configure
    println!("\n=== Step 0c: Extracting + configuring kernel ===");
    if build_dir.exists() {
        fs_err::remove_dir_all(&build_dir)?;
    }
    fs_err::create_dir_all(&build_dir)?;
    extract_tarball(&tarball, &build_dir)?;
    let kernel_src = build_dir.join(format!("linux-{}", version.linux_version));
    if !kernel_src.exists() {
        return Err(anyhow!(
            "expected extracted dir {} not found",
            kernel_src.display()
        ));
    }

    config::run_configure_phase(
        &tools_tree,
        &kernel_src,
        Path::new(REQUIRED_FRAGMENT),
        Path::new(HARDENING_FRAGMENT),
    )?;

    // Phase 0c.5: snapshot guard
    println!("\n=== Step 0c.5: Snapshot guard ===");
    let resolved = kernel_src.join(".config");
    let snapshot = Path::new(SNAPSHOT_PATH);
    if args.update_snapshot {
        config::update_snapshot(&resolved, snapshot)?;
        println!("snapshot updated: {}", snapshot.display());
    } else if !snapshot.exists() {
        return Err(anyhow!(
            "{} does not exist. Generate it with `steep kernel --update-snapshot`.",
            snapshot.display()
        ));
    } else {
        config::check_snapshot(&resolved, snapshot)?;
    }

    // Phase 0d: compile
    println!("\n=== Step 0d: Compiling kernel ===");
    compile::run(&tools_tree, &kernel_src, &vmlinuz_path, &log_path)?;

    // Phase 0e: finalize manifest
    println!("\n=== Step 0e: Writing manifest ===");
    let inputs = compute_fingerprint(&version, &tools_tree)?;
    let outputs = km::Outputs {
        vmlinuz_sha256: fetch::sha256_file(&vmlinuz_path)?,
    };
    let manifest = km::KernelManifest {
        version: 1,
        linux_version: version.linux_version.clone(),
        inputs,
        outputs,
        built_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    km::write(&manifest_path, &manifest)?;
    println!("kernel: {}", vmlinuz_path.display());
    println!("manifest: {}", manifest_path.display());
    Ok(())
}

/// Build the kernel-builder tools tree if needed, return its path.
fn ensure_tools_tree() -> Result<PathBuf> {
    let tree = Path::new(TOOLS_TREE_DIR).join("mkosi.output/image");
    let mkosi = tools::resolve_mkosi()?;
    tools::run_command_streaming(
        "sudo",
        &[mkosi.as_str(), "--directory", TOOLS_TREE_DIR, "--force"],
    )?;
    if !tree.exists() {
        return Err(anyhow!("mkosi did not produce {}", tree.display()));
    }
    Ok(tree.canonicalize()?)
}

/// Compute the fingerprint over all inputs that determine kernel build output.
pub fn compute_fingerprint(version: &KernelVersion, tools_tree: &Path) -> Result<km::Fingerprint> {
    Ok(km::Fingerprint {
        linux_version: version.linux_version.clone(),
        tarball_sha256: version.tarball_sha256.clone(),
        required_config_sha256: fetch::sha256_file(Path::new(REQUIRED_FRAGMENT))?,
        hardening_config_sha256: fetch::sha256_file(Path::new(HARDENING_FRAGMENT))?,
        snapshot_config_sha256: if Path::new(SNAPSHOT_PATH).exists() {
            fetch::sha256_file(Path::new(SNAPSHOT_PATH))?
        } else {
            String::new()
        },
        tools_tree_digest: tools_tree_digest(tools_tree)?,
    })
}

/// Hash the toolchain identity. We use the mkosi.conf bytes as a stable proxy:
/// the apt mirror snapshot URL is in there, package list is in there.
fn tools_tree_digest(_tools_tree: &Path) -> Result<String> {
    fetch::sha256_file(Path::new("mkosi/kernel-builder/mkosi.conf"))
}

fn extract_tarball(tarball: &Path, dest: &Path) -> Result<()> {
    tools::run_command_streaming(
        "tar",
        &[
            "--extract",
            "--xz",
            "--file",
            &tarball.to_string_lossy(),
            "--directory",
            &dest.to_string_lossy(),
        ],
    )?;
    Ok(())
}
```

- [ ] **Step 2: Remove the `_ref_config` shim from `src/kernel/compile.rs`**

Delete the trailing `#[allow(dead_code)] fn _ref_config() …` block from `src/kernel/compile.rs`. The unused-import warning it suppressed is now resolved by `commands::kernel::run` calling into `kernel::config`.

- [ ] **Step 3: Build**

```bash
cargo build
```

Expected: builds clean. The `args.force` field is unused — that gets wired in Task D3. Add `let _ = args.force;` at the top of `run` to silence the warning if needed.

- [ ] **Step 4: Commit**

```bash
git add src/commands/kernel.rs src/kernel/compile.rs
git commit -m "implement steep kernel command end to end"
```

---

## Task D3: `src/kernel_cache.rs` — `ensure_kernel`

**Files:**
- Modify: `src/kernel_cache.rs` (full rewrite)

- [ ] **Step 1: Write the module**

```rust
//! Cache-aware artifact accessor for the custom kernel build.
//!
//! The cache check lives in `commands::kernel::run`. This module is a thin
//! wrapper that calls the builder, reads the resulting manifest, and returns
//! a `KernelArtifact` shaped for use by `commands::build`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::commands;
use crate::kernel::manifest as km;
use crate::KernelArgs;

const SNAPSHOT_PATH: &str = "kernel/config-x86_64.snapshot";
const KERNEL_OUT_DIR: &str = "output/kernel";

pub struct KernelArtifact {
    pub vmlinuz_path: PathBuf,
    pub linux_version: String,
    pub manifest: km::KernelManifest,
}

/// Ensure a current kernel artifact exists at output/kernel/.
/// Force=true bypasses the cache (rebuilds from scratch).
pub fn ensure_kernel(force: bool) -> Result<KernelArtifact> {
    require_inputs_exist()?;

    commands::kernel::run(&KernelArgs {
        force,
        update_snapshot: false,
        output: PathBuf::from(KERNEL_OUT_DIR),
    })?;

    let manifest_path = Path::new(KERNEL_OUT_DIR).join("manifest.json");
    let vmlinuz_path = Path::new(KERNEL_OUT_DIR).join("vmlinuz");
    let manifest = km::read(&manifest_path)?;
    Ok(KernelArtifact {
        vmlinuz_path,
        linux_version: manifest.linux_version.clone(),
        manifest,
    })
}

fn require_inputs_exist() -> Result<()> {
    for f in ["kernel/version", "kernel/required.config", "kernel/hardening.config"] {
        if !Path::new(f).exists() {
            return Err(anyhow!("required file missing: {}", f));
        }
    }
    if !Path::new(SNAPSHOT_PATH).exists() {
        return Err(anyhow!(
            "{} missing. Run `steep kernel --update-snapshot` to generate.",
            SNAPSHOT_PATH
        ));
    }
    Ok(())
}
```

- [ ] **Step 2: Build**

```bash
cargo build
```

Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src/kernel_cache.rs
git commit -m "add kernel_cache::ensure_kernel"
```

---

## Task D4: Generate the initial snapshot

Until this point, `kernel/config-x86_64.snapshot` does not exist. We generate it now by running `steep kernel --update-snapshot` for real. This requires network (to fetch the tarball) and `systemd-nspawn`.

**Files:**
- Create: `kernel/config-x86_64.snapshot`
- Modify: `kernel/version` (replace placeholder SHA from Task A4 if needed)

- [ ] **Step 1: Verify the SHA in `kernel/version` is correct**

Cross-reference `kernel/version`'s `LINUX_TARBALL_SHA256` against `https://cdn.kernel.org/pub/linux/kernel/v6.x/sha256sums.asc`. If the SHA is wrong (placeholder from Task A4), edit `kernel/version` to the correct value before proceeding. Do not commit yet.

- [ ] **Step 2: Run the kernel build with snapshot update**

```bash
cargo build
sudo target/debug/steep kernel --update-snapshot
```

Expected: full kernel build runs (10–30 minutes on first build), `kernel/config-x86_64.snapshot` is created/overwritten, `output/kernel/vmlinuz` and `output/kernel/manifest.json` are created.

If the build fails: read `output/kernel/build.log`. Common issues:
- Missing kbuild dep → add to `mkosi/kernel-builder/mkosi.conf` Packages list, rebuild.
- Tarball SHA mismatch → fix `kernel/version`.

- [ ] **Step 3: Sanity-check the snapshot**

```bash
wc -l kernel/config-x86_64.snapshot
grep -c '^CONFIG_' kernel/config-x86_64.snapshot
grep '# CONFIG_MODULES is not set' kernel/config-x86_64.snapshot
grep '# CONFIG_SERIAL_8250 is not set' kernel/config-x86_64.snapshot
grep 'CONFIG_VIRTIO_CONSOLE=y' kernel/config-x86_64.snapshot
grep 'CONFIG_LOCK_DOWN_KERNEL_FORCE_CONFIDENTIALITY=y' kernel/config-x86_64.snapshot
```

Expected: ~5000 lines, most starting with `CONFIG_` or `# CONFIG_`; the four checks above print matching lines.

- [ ] **Step 4: Run the second time to verify cache hit**

```bash
sudo target/debug/steep kernel
```

Expected: prints `kernel cache HIT (linux <version>, sha256 <hex>)` and returns in seconds. The fingerprint short-circuit in `commands::kernel::run` (Task D2) skips Phases 0a–0e entirely on cache hit.

If cache hit isn't observed, debug the short-circuit: log `live` vs `cached.inputs` from Task D2's check; the divergence will pinpoint which fingerprint field differs across runs.

- [ ] **Step 5: Commit**

```bash
git add kernel/config-x86_64.snapshot
# Only commit kernel/version if Step 1 modified it.
git status kernel/version
git add kernel/version  # only if modified
git commit -m "generate initial kernel/config-x86_64.snapshot"
```

---

## Task D5: Wire `steep build` Phase 1 (kernel-cache check + pre-stage)

**Files:**
- Modify: `src/commands/build.rs`

- [ ] **Step 1: Add kernel cache check at the top of `run`**

In `src/commands/build.rs`, find the line `tracing::info!("sealing base image with dm-verity + UKI");` (line 9). Immediately after, before any other logic, add:

```rust
    // Phase 1: ensure custom kernel artifact is current
    println!("\n=== Step 1/4: Ensuring custom kernel ===");
    let kernel = crate::kernel_cache::ensure_kernel(false)?;
    println!(
        "kernel: {} (linux {})",
        kernel.vmlinuz_path.display(),
        kernel.linux_version
    );
```

- [ ] **Step 2: Renumber existing step banners**

In `src/commands/build.rs`:
- `Step 1/3: Building verity initrd (mkosi)` → `Step 2/4: Building verity initrd (mkosi)`
- `Step 2/3: Building image with mkosi (verity + UKI)` → `Step 3/4: Building image with mkosi (verity + UKI)`
- `Step 3/3: Building IGVM` → `Step 4/4: Building IGVM`
- `Step 3/3: Skipping IGVM (--skip-igvm)` → `Step 4/4: Skipping IGVM (--skip-igvm)`

- [ ] **Step 3: Pre-stage the kernel into mkosi.extra with RAII cleanup**

After the existing `_cloud_init_guard` setup (around line 73 in the current file, just before `// Step 1: Build the verity initrd via mkosi (declarative)`), add:

```rust
    // Pre-stage the custom kernel into mkosi.extra so mkosi finds it during UKI assembly.
    let staged_kernel_dir = PathBuf::from("mkosi/base/mkosi.extra/usr/lib/modules")
        .join(&kernel.linux_version);
    fs_err::create_dir_all(&staged_kernel_dir)?;
    let staged_kernel = staged_kernel_dir.join("vmlinuz");
    fs_err::copy(&kernel.vmlinuz_path, &staged_kernel)?;
    let _kernel_stage_guard = KernelStageCleanup {
        staged: staged_kernel,
    };
```

- [ ] **Step 4: Add the cleanup struct at the bottom of the file**

Append after `CloudInitCleanup` (after line 370 in the current file):

```rust
/// RAII guard that removes the pre-staged vmlinuz and prunes empty parent dirs
/// back up to mkosi.extra/. Mirrors CloudInitCleanup's behavior.
struct KernelStageCleanup {
    staged: PathBuf,
}

impl Drop for KernelStageCleanup {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.staged);
        let mut dir = self.staged.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            if d.ends_with("mkosi.extra") {
                break;
            }
            if fs_err::remove_dir(&d).is_err() {
                break;
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
}
```

- [ ] **Step 5: Build**

```bash
cargo build
```

Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add src/commands/build.rs
git commit -m "wire steep build to use cached custom kernel"
```

---

## Task D6: Add `kernel` block to build manifest

**Files:**
- Modify: `src/manifest.rs`
- Modify: `src/commands/build.rs`

- [ ] **Step 1: Extend `ManifestInputs`**

In `src/manifest.rs`, replace the existing `ManifestInputs`:

```rust
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInputs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel: Option<KernelInputs>,
    pub initrd: FileEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FileEntry>,
    pub base_image: FileEntry,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KernelInputs {
    pub linux_version: String,
    pub vmlinuz_sha256: String,
    pub required_config_sha256: String,
    pub hardening_config_sha256: String,
    pub snapshot_config_sha256: String,
}
```

- [ ] **Step 2: Populate it in `commands::build::run`**

Inside `src/commands/build.rs` `run`, find the `ManifestInputs { ... }` literal in the `BuildManifest` construction. Add the `kernel` field:

```rust
        inputs: ManifestInputs {
            kernel: Some(crate::manifest::KernelInputs {
                linux_version: kernel.linux_version.clone(),
                vmlinuz_sha256: kernel.manifest.outputs.vmlinuz_sha256.clone(),
                required_config_sha256: kernel.manifest.inputs.required_config_sha256.clone(),
                hardening_config_sha256: kernel.manifest.inputs.hardening_config_sha256.clone(),
                snapshot_config_sha256: kernel.manifest.inputs.snapshot_config_sha256.clone(),
            }),
            initrd: FileEntry { /* unchanged */
                path: initrd_path.to_string_lossy().to_string(),
                sha256: initrd_hash,
            },
            // ... rest unchanged
```

- [ ] **Step 3: Build**

```bash
cargo build
```

Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add src/manifest.rs src/commands/build.rs
git commit -m "include kernel block in build manifest"
```

---

# Phase E — Coordinated changes

## Task E1: `mkosi/base/mkosi.conf` — drop `linux-generic`, modules, switch console

**Files:**
- Modify: `mkosi/base/mkosi.conf`

- [ ] **Step 1: Remove `linux-generic` from the `Packages=` list**

Find the line `    linux-generic` and delete it.

- [ ] **Step 2: Remove `KernelModulesInitrd*` directives**

Delete lines:

```
KernelModulesInitrd=yes
KernelModulesInitrdInclude=
    dm-bufio
    dm-verity
    erofs
    overlay
    virtio_blk
    virtio_net
    virtio_pci
```

- [ ] **Step 3: Change `KernelCommandLine`**

```
- KernelCommandLine=console=ttyS0 earlyprintk=serial systemd.condition-first-boot=no
+ KernelCommandLine=console=hvc0 systemd.condition-first-boot=no
```

- [ ] **Step 4: Verify mkosi parses it**

```bash
mkosi --directory mkosi/base summary | head -40
```

Expected: prints summary, no errors.

- [ ] **Step 5: Commit**

```bash
git add mkosi/base/mkosi.conf
git commit -m "drop linux-generic and modules; switch console to hvc0"
```

---

## Task E2: Strip `kmod` and module loading from initrd

**Files:**
- Modify: `mkosi/initrd/mkosi.conf`
- Modify: `mkosi/initrd/mkosi.extra/init`

- [ ] **Step 1: Drop `kmod` from initrd Packages**

In `mkosi/initrd/mkosi.conf`, delete the `kmod` line from the `Packages=` block.

- [ ] **Step 2: Remove `depmod` + `modprobe` block**

In `mkosi/initrd/mkosi.extra/init`, delete lines 9–18 (the entire block from `echo "initrd: loading modules..."` through the `done`):

```bash
echo "initrd: loading modules..."
# depmod to build module dependency map (needed for modprobe in initrd)
depmod -a
for mod in dm-verity overlay; do
    if modprobe "$mod" 2>/dev/null; then
        echo "initrd: loaded $mod"
    else
        echo "initrd: WARNING: $mod not found"
    fi
done
```

The next line should now be `# Parse roothash from kernel cmdline (disable globbing to prevent expansion)`. Verify.

- [ ] **Step 3: Commit**

```bash
git add mkosi/initrd/mkosi.conf mkosi/initrd/mkosi.extra/init
git commit -m "drop kmod and module-loading from initrd"
```

---

## Task E3: QEMU virtio-console wiring (TDD)

**Files:**
- Modify: `src/qemu.rs`
- Modify: `tests/qemu.rs`

- [ ] **Step 1: Add a failing test**

Append to `tests/qemu.rs`:

```rust
#[test]
fn test_qemu_args_uses_virtio_console() {
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
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("virtio-serial-pci"), "missing virtio-serial-pci");
    assert!(joined.contains("virtconsole"), "missing virtconsole device");
    assert!(joined.contains("chardev=hvc0"), "missing hvc0 chardev hookup");
    // 8250 is gone — the SNP tier no longer uses -serial mon:stdio.
    assert!(!joined.contains("-serial mon:stdio"));
}

#[test]
fn test_qemu_args_kvm_uses_virtio_console() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: Some(PathBuf::from("/usr/share/OVMF/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("virtio-serial-pci"));
    assert!(joined.contains("chardev=hvc0"));
}
```

- [ ] **Step 2: Run tests; expect failures**

```bash
cargo test --test qemu test_qemu_args_uses_virtio_console
```

Expected: both new tests fail.

- [ ] **Step 3: Modify SevSnp tier args in `src/qemu.rs`**

In `src/qemu.rs`, find the SevSnp tier vec construction (the block that includes `"-serial".to_string(), "mon:stdio".to_string(), "-monitor".to_string(), "none".to_string()`). Replace those four entries with the virtio-console block:

```rust
                    "-no-reboot".to_string(),
                    "-chardev".to_string(),
                    "stdio,id=hvc0,signal=off".to_string(),
                    "-device".to_string(),
                    "virtio-serial-pci,id=virtser0".to_string(),
                    "-device".to_string(),
                    "virtconsole,chardev=hvc0,id=console0".to_string(),
                    "-monitor".to_string(),
                    "none".to_string(),
                ]
```

(Keep `-no-reboot`; replace the four `-serial`/`mon:stdio` lines.)

- [ ] **Step 4: Add the same block to Kvm/Emulated tiers**

In the same file, the Kvm/Emulated branch builds `v` then extends it with `-drive`/`-kernel` etc. After the existing `v.extend([...])` push the virtio-console additions:

```rust
                v.extend([
                    "-chardev".to_string(),
                    "stdio,id=hvc0,signal=off".to_string(),
                    "-device".to_string(),
                    "virtio-serial-pci,id=virtser0".to_string(),
                    "-device".to_string(),
                    "virtconsole,chardev=hvc0,id=console0".to_string(),
                ]);
```

- [ ] **Step 5: Re-run tests**

```bash
cargo test --test qemu
```

Expected: full qemu test suite passes (existing + new).

- [ ] **Step 6: Commit**

```bash
git add src/qemu.rs tests/qemu.rs
git commit -m "switch qemu console wiring from ttyS0 to hvc0"
```

---

## Task E4: `--console` autologin path → hvc0

**Files:**
- Modify: `src/commands/build.rs`

- [ ] **Step 1: Find the autologin path**

In `src/commands/build.rs`, find the line:

```rust
    let autologin_dir =
        PathBuf::from("mkosi/base/mkosi.extra/etc/systemd/system/serial-getty@ttyS0.service.d");
```

- [ ] **Step 2: Replace ttyS0 with hvc0**

```rust
    let autologin_dir =
        PathBuf::from("mkosi/base/mkosi.extra/etc/systemd/system/serial-getty@hvc0.service.d");
```

- [ ] **Step 3: Update the warning text**

Find the line `println!("WARNING: --console enables passwordless root on serial console. Do not use in production.");` and update if it mentions ttyS0; otherwise no change.

- [ ] **Step 4: Build**

```bash
cargo build
```

Expected: builds clean.

- [ ] **Step 5: Commit**

```bash
git add src/commands/build.rs
git commit -m "update --console autologin path to hvc0"
```

---

## Task E5: Update `tests/e2e.sh` for hvc0

**Files:**
- Modify: `tests/e2e.sh`

- [ ] **Step 1: Update the cloud-init heredoc**

In `tests/e2e.sh`, find:

```bash
runcmd:
  - |
    exec > /dev/ttyS0 2>&1
```

Replace with:

```bash
runcmd:
  - |
    exec > /dev/hvc0 2>&1
```

- [ ] **Step 2: Update the QEMU launch command in Test 4**

The hand-rolled qemu-system-x86_64 invocation currently uses default `-nographic` serial. Replace the relevant flag block with virtio-console wiring:

Find:

```bash
    qemu-system-x86_64 \
        -machine q35 \
        -enable-kvm \
        -drive "if=pflash,format=raw,readonly=on,file=$BOOT_FW" \
        -kernel "$OUT/uki.efi" \
        -drive "file=$OUT/disk.raw,format=raw,if=virtio" \
        -smp 1 -m 4G \
        -nographic \
        -no-reboot \
        -netdev "user,id=net0,hostfwd=tcp::${HOST_PORT}-:${GUEST_PORT}" \
        -device virtio-net-pci,netdev=net0 \
        </dev/null \
        > "$SERIAL_LOG" 2>&1 &
```

Replace `-nographic` line with `-nographic -chardev "stdio,id=hvc0,signal=off" -device "virtio-serial-pci,id=virtser0" -device "virtconsole,chardev=hvc0,id=console0"`. Final block:

```bash
    qemu-system-x86_64 \
        -machine q35 \
        -enable-kvm \
        -drive "if=pflash,format=raw,readonly=on,file=$BOOT_FW" \
        -kernel "$OUT/uki.efi" \
        -drive "file=$OUT/disk.raw,format=raw,if=virtio" \
        -smp 1 -m 4G \
        -nographic \
        -chardev "stdio,id=hvc0,signal=off" \
        -device "virtio-serial-pci,id=virtser0" \
        -device "virtconsole,chardev=hvc0,id=console0" \
        -no-reboot \
        -netdev "user,id=net0,hostfwd=tcp::${HOST_PORT}-:${GUEST_PORT}" \
        -device virtio-net-pci,netdev=net0 \
        </dev/null \
        > "$SERIAL_LOG" 2>&1 &
```

- [ ] **Step 3: Note any references to `seal`**

The current `tests/e2e.sh` references `steep seal` (e.g., `$STEEP seal --skip-igvm ...`). Per recent commit history, `seal` was merged into `build`. If this prevents the script from running at all (separate concern from this plan), file a note as a TODO comment at the top of the script. Do not fix in this task — out of scope.

- [ ] **Step 4: Commit**

```bash
git add tests/e2e.sh
git commit -m "update e2e to use hvc0 virtio-console"
```

---

# Phase F — Integration tests + final verification

## Task F1: Integration tests for kernel cache + reproducibility

**Files:**
- Create: `tests/kernel.rs`

- [ ] **Step 1: Write the test file**

```rust
//! Integration tests for steep kernel + kernel_cache.
//!
//! These run a real kernel build under systemd-nspawn. Mark with `#[ignore]`
//! so `cargo test` doesn't trigger a 10+ minute build.
//!
//! Run with: `cargo test --test kernel -- --ignored`
//!
//! Requires:
//!   - `kernel/version`, `kernel/required.config`, `kernel/hardening.config`,
//!     `kernel/config-x86_64.snapshot` checked in
//!   - `mkosi/kernel-builder/` config in place
//!   - sudo + systemd-nspawn available
//!   - network access to cdn.kernel.org
//!
//! Each test runs in its own temp output dir (no interference with `output/kernel/`).

use assert_cmd::Command;
use std::path::PathBuf;

fn binary() -> PathBuf {
    assert_cmd::cargo::cargo_bin("steep")
}

#[test]
#[ignore]
fn kernel_build_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();
    assert!(out.join("vmlinuz").exists());
    assert!(out.join("manifest.json").exists());
}

#[test]
#[ignore]
fn kernel_build_is_reproducible() {
    let tmp1 = tempfile::TempDir::new().unwrap();
    let tmp2 = tempfile::TempDir::new().unwrap();

    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(tmp1.path().join("kernel"))
        .assert()
        .success();
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(tmp2.path().join("kernel"))
        .assert()
        .success();

    let h1 = sha256(&tmp1.path().join("kernel/vmlinuz"));
    let h2 = sha256(&tmp2.path().join("kernel/vmlinuz"));
    assert_eq!(h1, h2, "vmlinuz not reproducible across builds");
}

#[test]
#[ignore]
fn kernel_cache_hits_on_second_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();

    let m1 = std::fs::metadata(out.join("vmlinuz")).unwrap();
    let mtime1 = m1.modified().unwrap();
    // Sleep a second so any rebuild has a strictly later mtime.
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Second run with the same output dir should hit cache.
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();
    let m2 = std::fs::metadata(out.join("vmlinuz")).unwrap();
    let mtime2 = m2.modified().unwrap();
    assert_eq!(mtime1, mtime2, "vmlinuz was rewritten — cache miss when hit expected");
}

#[test]
#[ignore]
fn kernel_drift_fails_without_update_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let out = tmp.path().join("kernel");
    // First, do a clean build to populate the cache + snapshot.
    Command::new(binary())
        .args(["kernel", "--output"])
        .arg(&out)
        .assert()
        .success();

    // Modify the snapshot so the next build's resolved config diverges.
    let snap = std::fs::read_to_string("kernel/config-x86_64.snapshot").unwrap();
    let modified = format!("{}\n# DRIFT_TEST_MARKER=1\n", snap);
    let backup = std::fs::read("kernel/config-x86_64.snapshot").unwrap();
    std::fs::write("kernel/config-x86_64.snapshot", modified).unwrap();
    let result = Command::new(binary())
        .args(["kernel", "--force", "--output"])
        .arg(&out)
        .output()
        .unwrap();
    // Restore the snapshot regardless of test outcome.
    std::fs::write("kernel/config-x86_64.snapshot", backup).unwrap();

    assert!(!result.status.success(), "expected drift to fail");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(".config drift") || stderr.contains("update-snapshot"));
}

fn sha256(p: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).unwrap();
    hex::encode(h.finalize())
}
```

- [ ] **Step 2: Run the integration tests (slow!)**

```bash
sudo -E cargo test --test kernel -- --ignored
```

Expected: all four tests pass. Total runtime: 30+ minutes on first kernel build, less for cache-hit cases.

If `kernel_drift_fails` mutates `kernel/config-x86_64.snapshot` and the test fails before restoring, manually run `git checkout kernel/config-x86_64.snapshot` to recover.

- [ ] **Step 3: Commit**

```bash
git add tests/kernel.rs
git commit -m "add kernel integration tests"
```

---

## Task F2: Full `steep build` end-to-end

**Files:** none modified — verification only.

- [ ] **Step 1: Run `cargo test`**

```bash
cargo test
```

Expected: all unit tests + non-ignored integration tests pass.

- [ ] **Step 2: Run `steep build` end-to-end**

```bash
cargo build
sudo target/debug/steep build --skip-igvm --memory 4G
```

Expected:
- Banner shows `Step 1/4`, `Step 2/4`, `Step 3/4`, `Step 4/4` (or `Step 4/4: Skipping IGVM`).
- Kernel cache HIT (since snapshot already built in D4).
- Build completes.
- `output/base/manifest.json` contains a non-null `inputs.kernel` block with `linux_version`, `vmlinuz_sha256`, etc.

- [ ] **Step 3: Sanity-check the staged kernel was cleaned up**

```bash
test -d mkosi/base/mkosi.extra/usr/lib/modules && echo "LEAKED" || echo "clean"
```

Expected: `clean`. The RAII guard prunes the directory.

- [ ] **Step 4: Optional — boot the resulting image**

```bash
sudo target/debug/steep run output/base
```

Expected: hvc0 console output reaches stdout, systemd reaches multi-user target. `cat /proc/sys/kernel/lockdown` inside the guest reports `[confidentiality]`. Press `Ctrl-A X` (or `Ctrl-]`) to exit QEMU. (If QEMU monitor is disabled and Ctrl-C doesn't work, the test harness's stdio mux interferes — kill from another terminal.)

- [ ] **Step 5: Commit any test-driven fixes**

If steps 1-4 surfaced bugs (missing config, wrong path, etc.), commit the fixes:

```bash
git add <files>
git commit -m "<description of fix>"
```

If everything passed cleanly, no commit here.

---

## Task F3: Push the branch and review

**Files:** none

- [ ] **Step 1: Run `cargo fmt` and `cargo clippy`**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

Expected: no output (clean) or only pre-existing warnings unrelated to this branch.

If formatting changes occurred:

```bash
git add -u
git commit -m "cargo fmt"
```

- [ ] **Step 2: Check the commit log**

```bash
git log --oneline main..HEAD
```

Expected: ~25 commits, each one a small focused change matching the tasks above.

- [ ] **Step 3: Push**

```bash
git push -u origin kernel-subcommand
```

- [ ] **Step 4: Open a PR**

Title: `Add steep kernel subcommand for hardened reproducible kernels`

Body: link to the spec at `docs/superpowers/specs/2026-04-29-steep-kernel-design.md`; mention the coordinated changes (no-modules, hvc0); flag follow-ups (`README.md` and `docs/CONCEPTS.md` are stale per spec).

---

# Self-review notes (author)

**Spec coverage:**
- `steep kernel` CLI surface — Task D1.
- Pipeline phases 0a–0e — Tasks D2 (orchestration), C1 (fetch), C2 (configure), C3 (compile), B3 (manifest).
- `kernel/version`, fragments, snapshot — Tasks A2/A3/A4/D4.
- `mkosi/kernel-builder/` — Task A5.
- Reproducibility env vars — Task C3.
- Cache fingerprint — Task B3 + D2 (`compute_fingerprint`).
- Snapshot guard — Tasks B4 (logic) + D2 (call site).
- `--update-snapshot` flow — Task D2 + D4.
- `steep build` Phase 0 + RAII — Task D5.
- Build manifest kernel block — Task D6.
- `mkosi/base/mkosi.conf` changes — Task E1.
- Initrd changes — Task E2.
- QEMU virtio-console — Task E3.
- `--console` autologin path — Task E4.
- `tests/e2e.sh` updates — Task E5.
- Integration tests (succeeds/reproducible/cache-hits/drift-fails) — Task F1.
- E2E verification — Task F2.

**Placeholder scan:** Task A4 explicitly notes a TBD SHA256 that's resolved in Task D4 — that's an intentional staged sequence, not a real placeholder. No `TBD`/`TODO` left in code; all code blocks are complete.

**Type consistency:** `KernelArtifact`, `KernelManifest`, `Fingerprint`, `KernelInputs`, `KernelArgs` names are stable across tasks. `commands::kernel::compute_fingerprint` is referenced from `kernel_cache::ensure_kernel` with matching signature.

**Out-of-plan caveats:**
- `tests/e2e.sh` still uses `steep seal` (renamed to `build` per recent commits); a TODO comment is added but the broader script-update is out of scope.
- The "hvc0 vs ttyS0" discovery in QEMU's stdio mux may surface during F2 booting — the `signal=off` chardev option mitigates Ctrl-C interference. If broken UX shows up, the fix lives in `src/qemu.rs` Task E3 (one-line change to chardev options).
