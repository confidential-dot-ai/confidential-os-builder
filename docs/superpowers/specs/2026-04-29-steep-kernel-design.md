# `steep kernel` — hardened custom kernel design

## Goal

Add a `steep kernel` subcommand that builds a hardened, reproducible Linux kernel from
upstream source, and have `steep build` consume that kernel as part of the image
pipeline. The kernel becomes part of the existing measurement chain
(roothash → UKI → IGVM → SNP launch digest).

## Non-goals

- Replacing mkosi's UKI assembly. mkosi continues to assemble the UKI; we only swap
  out the kernel input.
- Cross-compiling (x86_64 only).
- Module signing infrastructure. `CONFIG_MODULES` is off; signing is moot.
- Closing the existing apt-snapshot trust gap. The kernel-builder reuses the same
  Ubuntu snapshot pin as the base; cross-machine reproducibility inherits whatever
  guarantee that pin already provides.
- Updating prose docs (`README.md`, `docs/CONCEPTS.md`) — listed as follow-up work,
  not blocking on this spec.

## Constraints

- Must produce bit-identical `vmlinuz` across builds with identical inputs. The
  project's value proposition depends on reproducible IGVM measurements.
- Kernel hardening posture: KSPP-style + confidential-computing-tightening, using
  the operator-supplied config fragment.
- Build runs inside an mkosi-managed toolchain tree pinned to
  `snapshot.ubuntu.com/ubuntu/20260405T000000Z` — the same mirror snapshot the base
  and initrd already use.
- Source is fetched from `cdn.kernel.org`, version + SHA256 pinned in the repo.
- `steep build` auto-builds the kernel if cache is stale; users normally never
  invoke `steep kernel` directly. The command stays `#[command(hide = true)]`.
- No runtime modules: the entire kernel is built-in (`# CONFIG_MODULES is not set`,
  enforced by `mod2yesconfig` in the pipeline as defense-in-depth).
- No 8250 serial: the build switches the console to virtio-console (`hvc0`).

---

## Architecture

### New artifacts

```
kernel/
  version                    # LINUX_VERSION + LINUX_TARBALL_SHA256
  required.config            # functional configs steep needs to boot
  hardening.config           # security posture (operator-supplied fragment)
  config-x86_64.snapshot     # full resolved .config; audit artifact

mkosi/kernel-builder/
  mkosi.conf                 # toolchain image (gcc, binutils, flex, bison,
                             # bc, libelf-dev, libssl-dev, xz-utils, cpio, make)
  mkosi.output/              # mkosi-managed; git-ignored

output/kernel/               # local cache; git-ignored
  cache/linux-<ver>.tar.xz   # downloaded tarball, SHA-verified
  build/                     # extracted kernel tree
  build.log                  # truncated each invocation
  vmlinuz                    # final artifact
  manifest.json              # inputs + outputs

src/kernel_cache.rs          # fingerprint + cache lookup
src/commands/kernel.rs       # CLI entry point (currently a TODO stub)
```

### Modified artifacts

- `src/lib.rs` — `KernelArgs` reshaped (drop `--source`, `--config`; add
  `--force`, `--update-snapshot`, keep `--output` with default `output/kernel`).
- `src/main.rs` — unchanged dispatch; subcommand stays hidden.
- `src/commands/build.rs` — new Phase 0 invokes `kernel_cache::ensure_kernel`,
  pre-stages `vmlinuz`, then proceeds through existing phases. Step counters
  bump to N/4. The `--console` autologin drop-in path moves from
  `serial-getty@ttyS0.service.d/` to `serial-getty@hvc0.service.d/`.
- `src/qemu.rs` — replaces `-serial mon:stdio` with virtio-console wiring across
  all three tiers (SevSnp, Kvm, Emulated).
- `mkosi/base/mkosi.conf` — drops `linux-generic` from `Packages=`, drops
  `KernelModulesInitrd*` directives, changes `KernelCommandLine` to
  `console=hvc0 systemd.condition-first-boot=no`.
- `mkosi/initrd/mkosi.conf` — drops `kmod` from `Packages=`.
- `mkosi/initrd/mkosi.extra/init` — removes the `depmod -a` and `modprobe` loop
  (lines 9–18 in the current script). Everything those load is built-in.
- `tests/qemu.rs`, `tests/e2e.sh` — assertions move from ttyS0 to hvc0.

### Data flow at build time

```
steep build
  │
  ├─ Phase 0: ensure-kernel  (kernel_cache::ensure_kernel)
  │    Compute fingerprint:
  │      - linux_version             from kernel/version
  │      - tarball_sha256             from kernel/version
  │      - required_config_sha256     sha256(kernel/required.config)
  │      - hardening_config_sha256    sha256(kernel/hardening.config)
  │      - snapshot_config_sha256     sha256(kernel/config-x86_64.snapshot)
  │      - tools_tree_digest          sha256(mkosi/kernel-builder/mkosi.conf
  │                                          + mkosi.tools.manifest, if present)
  │    If output/kernel/manifest.json.inputs == fingerprint and
  │       sha256(output/kernel/vmlinuz) == manifest.outputs.vmlinuz_sha256:
  │      cache HIT, return path
  │    Else:
  │      delegate to commands::kernel::run (Phases 0a–0e below)
  │
  │    Phase 0a: build kernel-builder tools tree (mkosi). mkosi handles its own
  │              cache; idempotent on unchanged inputs.
  │    Phase 0b: fetch
  │              GET https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-${VER}.tar.xz
  │              verify SHA256; cache to output/kernel/cache/.
  │    Phase 0c: configure (inside nspawn)
  │              extract tarball to output/kernel/build/
  │              make x86_64_defconfig
  │              scripts/kconfig/merge_config.sh -m .config kernel/required.config
  │              scripts/kconfig/merge_config.sh -m .config kernel/hardening.config
  │              make mod2yesconfig
  │              make olddefconfig
  │    Phase 0c.5: snapshot guard
  │              diff -q .config kernel/config-x86_64.snapshot
  │              if differ: with --update-snapshot, overwrite + continue;
  │                         else bail with diff path.
  │    Phase 0d: compile (inside nspawn)
  │              env: SOURCE_DATE_EPOCH=0
  │                   KBUILD_BUILD_TIMESTAMP=@0
  │                   KBUILD_BUILD_USER=steep
  │                   KBUILD_BUILD_HOST=steep
  │                   KCONFIG_NOTIMESTAMP=1
  │              make -j$(available_parallelism) bzImage
  │              tee stdout/stderr to output/kernel/build.log
  │    Phase 0e: finalize
  │              copy arch/x86/boot/bzImage → output/kernel/vmlinuz
  │              compute sha256, write output/kernel/manifest.json
  │
  ├─ Phase 1 (was: setup): pre-stage vmlinuz
  │    cp output/kernel/vmlinuz \
  │       mkosi/base/mkosi.extra/usr/lib/modules/${LINUX_VERSION}/vmlinuz
  │    RAII guard removes on drop, prunes empty parent dirs.
  │
  ├─ Phase 2: build verity initrd via mkosi             (existing logic, unchanged)
  ├─ Phase 3: build base image via mkosi                (existing logic; mkosi
  │           picks up the staged vmlinuz, assembles UKI from it + initrd
  │           + roothash cmdline)
  └─ Phase 4: assemble IGVM                              (existing logic)
```

---

## CLI

```rust
pub struct KernelArgs {
    /// Force rebuild even if cache is current
    #[arg(short, long)]
    pub force: bool,

    /// Regenerate kernel/config-x86_64.snapshot from defconfig + fragments.
    /// Use after bumping the kernel version or editing hardening.config.
    #[arg(long)]
    pub update_snapshot: bool,

    /// Output directory.
    #[arg(short, long, default_value = "output/kernel")]
    pub output: PathBuf,
}
```

`steep kernel` runs Phases 0a–0e directly. `steep build` invokes the same code
path through `kernel_cache::ensure_kernel` so artifacts are byte-identical
regardless of how the build is triggered. Subcommand stays
`#[command(hide = true)]` — escape hatch, not routine UX.

---

## `kernel/` files

### `kernel/version`

Two-line file, KEY=VALUE:

```
LINUX_VERSION=6.12.7
LINUX_TARBALL_SHA256=<64 hex chars>
```

The exact version is set during implementation. Version bumps are operator-driven:
edit this file, run `steep kernel --update-snapshot`, commit the resulting
snapshot diff alongside.

### `kernel/required.config`

Functional configs the steep image needs to boot — independent of any security
posture. Curated based on what the rootfs and initrd actually use:

```
# Filesystems we mount
CONFIG_EROFS_FS=y
CONFIG_OVERLAY_FS=y
CONFIG_EXT4_FS=y
CONFIG_TMPFS=y

# Device mapper + verity
CONFIG_BLK_DEV_DM=y
CONFIG_DM_VERITY=y
CONFIG_DM_BUFIO=y

# SEV-SNP guest
CONFIG_AMD_MEM_ENCRYPT=y
CONFIG_X86_MEM_ENCRYPT=y
CONFIG_AMD_MEM_ENCRYPT_ACTIVE_BY_DEFAULT=y

# Init / cmdline parsing
CONFIG_DEVTMPFS=y
CONFIG_DEVTMPFS_MOUNT=y
```

Final list verified during Phase 0c — `make olddefconfig` + the snapshot guard
catch anything missing or wrong.

### `kernel/hardening.config`

Operator-supplied fragment, deduped and normalized from the originally pasted list.
Includes:

- Minimal virtio set: `VIRTIO`, `VIRTIO_PCI`, `VIRTIO_BLK`, `VIRTIO_NET`,
  `VIRTIO_CONSOLE`, `VIRTIO_VSOCKETS`.
- Disabled attack surfaces: `USB_SUPPORT`, `SOUND`, `DRM`, `WIRELESS`, `BLUETOOTH`,
  `INPUT_MISC`, `HW_RANDOM_VIRTIO`, `NET_9P`, `HOTPLUG_PCI*`,
  `PCI_QUIRKS`, `PCI_ATS`, `PCI_IOV`, `PCI_PRI`, `PCI_PASID`, `PCI_LABEL`,
  `IOMMU_SVA`.
- RDRAND-trusted RNG: `ARCH_RANDOM`, `RANDOM_TRUST_CPU`.
- Lockdown: `SECURITY_LOCKDOWN_LSM`, `SECURITY_LOCKDOWN_LSM_EARLY`,
  `LOCK_DOWN_KERNEL_FORCE_CONFIDENTIALITY`.
- Memory safety: `STACKPROTECTOR`, `STACKPROTECTOR_STRONG`, `HARDENED_USERCOPY`,
  `SLAB_FREELIST_RANDOM`, `SLAB_FREELIST_HARDENED`.
- ACPI minimization: `ACPI_DOCK`, `ACPI_PROCESSOR`, `ACPI_HOTPLUG_CPU`,
  `ACPI_HOTPLUG_MEMORY`, `ACPI_CUSTOM_DSDT`, `ACPI_DEBUG`, `ACPI_PCI_SLOT`,
  `ACPI_BGRT`, `ACPI_TABLE_UPGRADE` all unset.
- Port-IO surface reduction: `SERIAL_8250`, `KEYBOARD_ATKBD`, `SERIO_I8042`,
  `RTC_DRV_CMOS`, `PCSPKR_PLATFORM` all unset.
- No modules: `# CONFIG_MODULES is not set`.
- No debug info: `# CONFIG_DEBUG_INFO is not set`,
  `# CONFIG_LOCALVERSION_AUTO is not set`.

The duplicate `PCI_ATS` / `PCI_PRI` / `PCI_PASID` lines in the original paste
are collapsed. Each grouping has a `#`-prefixed rationale comment above it.

### `kernel/config-x86_64.snapshot`

Full resolved `.config` (~5000 lines). Generated by Phase 0c, regenerated only
by `steep kernel --update-snapshot`. Reviewed in PRs.

---

## Cache fingerprint

The fingerprint is the canonical-JSON object that decides cache hit:

```json
{
  "linux_version":           "6.12.7",
  "tarball_sha256":          "...",
  "required_config_sha256":  "...",
  "hardening_config_sha256": "...",
  "snapshot_config_sha256":  "...",
  "tools_tree_digest":       "..."
}
```

Stored under `inputs` in `output/kernel/manifest.json`. `kernel_cache::ensure_kernel`
serializes the live fingerprint, compares, and either returns the cached
artifact or rebuilds. JSON is sorted by key with no whitespace.

---

## `output/kernel/manifest.json`

```json
{
  "version": 1,
  "linux_version": "6.12.7",
  "inputs": {
    "linux_version": "6.12.7",
    "tarball_sha256": "...",
    "required_config_sha256": "...",
    "hardening_config_sha256": "...",
    "snapshot_config_sha256": "...",
    "tools_tree_digest": "..."
  },
  "outputs": {
    "vmlinuz_sha256": "..."
  },
  "built_at": "2026-04-29T12:00:00Z"
}
```

`built_at` is informational only — not part of the fingerprint.

## Build manifest impact

`output/<name>/manifest.json` (the per-build manifest produced by
`commands::build::run`) gains a `kernel` block under `inputs`:

```json
"inputs": {
  "kernel": {
    "linux_version": "6.12.7",
    "vmlinuz_sha256": "...",
    "required_config_sha256": "...",
    "hardening_config_sha256": "...",
    "snapshot_config_sha256": "..."
  },
  "initrd": { ... },
  "firmware": { ... },
  "base_image": { ... }
}
```

The roothash and SNP launch digest already cover the kernel transitively
(it's bundled in the UKI which is bundled in the IGVM), but the explicit
block keeps audit-by-eyeball easy.

---

## Reproducibility

Sources of non-determinism and their mitigations:

| Source | Mitigation |
|---|---|
| `__DATE__`/`__TIME__` macros, modinfo timestamps | `KBUILD_BUILD_TIMESTAMP=@0` |
| Build host/user in version string | `KBUILD_BUILD_USER=steep`, `KBUILD_BUILD_HOST=steep` |
| `CONFIG_LOCALVERSION_AUTO` reading git | Force `# CONFIG_LOCALVERSION_AUTO is not set` in fragment |
| File mtimes from extraction | `SOURCE_DATE_EPOCH=0` |
| GCC/binutils version drift | Pinned via `mkosi/kernel-builder/` apt snapshot |
| Build paths in debug info | `# CONFIG_DEBUG_INFO is not set` |
| Module signing keys (random per build) | N/A — modules off |
| nspawn/host quirks | Bind-mount only the build dir + tools tree |
| kconfig auxiliary timestamps | `KCONFIG_NOTIMESTAMP=1` |

Out of scope:

- mkosi version pinning. Already handled by `bin/setup` (`uv tool install
  git+https://github.com/systemd/mkosi.git@v26`).
- Host kernel differences affecting nspawn semantics. Documented as a known
  caveat; verifiable by comparing `output/kernel/manifest.json` across machines.

---

## Coordinated changes (no-modules, no-8250)

### `mkosi/base/mkosi.conf`

- Remove `linux-generic` from `Packages=`. Kernel comes from
  `mkosi.extra/usr/lib/modules/${LINUX_VERSION}/vmlinuz`.
- Remove `KernelModulesInitrd=yes` and the entire `KernelModulesInitrdInclude=`
  block. No `.ko` files exist.
- `KernelCommandLine`:
  ```
  - console=ttyS0 earlyprintk=serial systemd.condition-first-boot=no
  + console=hvc0 systemd.condition-first-boot=no
  ```
  `earlyprintk` is dropped — the virtio-console early-printing path is a
  follow-up if pre-init output is needed.

### `mkosi/initrd/mkosi.conf`

- Drop `kmod` from `Packages=`.

### `mkosi/initrd/mkosi.extra/init`

- Delete the module-loading block (lines 9–18 in the current script):
  ```bash
  echo "initrd: loading modules..."
  depmod -a
  for mod in dm-verity overlay; do
      ...
  done
  ```
- All other logic (cmdline parse, veritysetup, overlay setup, switch_root)
  is unchanged.

### `src/qemu.rs`

For all three tiers (SevSnp, Kvm, Emulated), replace `-serial mon:stdio`-style
wiring with virtio-console:

```
-device virtio-serial-pci,id=virtser0
-chardev stdio,id=hvc0,signal=off
-device virtconsole,chardev=hvc0,id=console0
```

Kvm/Emulated tiers don't currently set `-serial` explicitly (they rely on
`-nographic`'s default), so they need the virtio-console block added.
`-nographic` stays. SNP tier's `-monitor none` stays.

### `src/commands/build.rs`

- New Phase 0 at top of `run()` calling `kernel_cache::ensure_kernel`. RAII
  guard pre-stages and cleans up the staged vmlinuz around the existing
  mkosi calls. Step banners renumber from N/3 to N/4.
- `--console` autologin drop-in path:
  ```
  - mkosi/base/mkosi.extra/etc/systemd/system/serial-getty@ttyS0.service.d/
  + mkosi/base/mkosi.extra/etc/systemd/system/serial-getty@hvc0.service.d/
  ```
  The `agetty` line keeps `%I` so it works generically.

### `mkosi/kernel-builder/mkosi.conf` (new)

Minimal sketch:

```
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
    flex
    gcc
    libelf-dev
    libssl-dev
    make
    perl
    rsync
    xz-utils

[Output]
Format=directory
Seed=d4f09d27-7e4e-4b1a-9c3a-deadbeef0003
```

Final package list refined during implementation; the kbuild dependency
list is well-documented upstream.

---

## Error handling

| Failure | Handling |
|---|---|
| `kernel/version` missing or malformed | `bail!` immediately, name the missing field |
| Tarball download fails (network, 404) | `bail!` with URL + curl exit; suggest checking `kernel/version` |
| Tarball SHA256 mismatch | `bail!` with both expected and actual hash; do not retry; do not delete the bad cached file |
| `merge_config.sh` warns about redefined symbols | Print warnings; do not fail (these happen legitimately) |
| `.config` differs from snapshot | `bail!` with `diff <snapshot> <build/.config>` and pointer to `--update-snapshot` |
| `make` non-zero exit | `bail!` with exit code and `Full log: output/kernel/build.log` |
| `vmlinuz` missing after `make` succeeds | `bail!` "kernel build claimed success but bzImage missing" |
| nspawn binary not in PATH | `bail!` "systemd-nspawn required; install systemd-container" |
| kernel-builder tools tree missing/stale | re-run `mkosi --directory mkosi/kernel-builder` automatically; bubble its failures |
| `output/kernel/vmlinuz` exists but hash differs from manifest | `bail!` "kernel artifact corrupted, run `steep kernel --force`" |
| `mkosi.extra/.../vmlinuz` already staged from prior run | `KernelStageCleanup` removes it before staging |

No silent fallbacks. Cache hits and rebuilds are decided by fingerprint, not
network reachability or filesystem state.

---

## Testing

### Unit tests (`tests/cli.rs` style — fast, no network, no nspawn)

- `kernel_cache::compute_fingerprint` — fixture inputs produce deterministic JSON.
- `kernel_cache::ensure_kernel` — fingerprint-match path returns cached artifact;
  mismatch path delegates to the build pipeline. Use a trait + mock for the
  actual build invocation.
- `kernel/version` parser — valid, missing-version-line, missing-sha-line,
  extra-whitespace cases.
- Snapshot diff guard — equal `.config`s pass; differing produce a structured
  error containing the diff path.
- Manifest JSON shape — golden file in `tests/fixtures/kernel_manifest.json`.

### Integration tests (`tests/kernel.rs`, new — slow, opt-in)

Behind `#[ignore]` so `cargo test` doesn't trigger a 10-minute build.
`cargo nextest --run-ignored=all` runs them in CI.

- `kernel_build_succeeds` — full `steep kernel` against a checked-in tiny test
  fragment that builds a minimal kernel; verifies `output/kernel/vmlinuz`
  exists, hash recorded, manifest valid.
- `kernel_build_is_reproducible` — run `steep kernel` twice in fresh dirs,
  assert byte-identical `vmlinuz` and identical `vmlinuz_sha256`. Load-bearing
  test for the project's reproducibility claim.
- `kernel_cache_hits` — second run with no input changes is a cache hit
  (no `make` invoked); use a marker file to detect re-entry.
- `kernel_drift_fails` — after a successful build, modify
  `kernel/hardening.config` without regenerating the snapshot. Re-run
  `steep kernel` and assert: cache miss triggers rebuild, Phase 0c.5 snapshot
  guard fires, command exits nonzero with the expected diff-pointer message.

### E2E (`tests/e2e.sh` extension)

After `steep build`, boot the resulting image under QEMU (KVM tier, no SNP)
and assert:

- hvc0 produces `systemd[1]: Reached target ...` output.
- `uname -r` matches `LINUX_VERSION` from `kernel/version`.
- `/proc/sys/kernel/lockdown` reports `[confidentiality]`.
- `/proc/modules` is empty.

### Existing tests requiring updates

- `tests/qemu.rs` — assertions on `-serial`-related flags become assertions
  on virtio-console flags.
- `tests/e2e.sh` — serial scraping moves from ttyS0 to hvc0. The harness still
  reads stdout the same way (`-chardev stdio,id=hvc0,...`).

### Out of scope for CI

- The actual SNP launch digest. Requires SNP hardware. The reproducibility test
  ensures the inputs are stable; verifying the digest itself is a separate
  operator concern.

---

## Open implementation details

These are not design questions — just things to confirm during implementation:

- Exact kernel version + tarball SHA. Pick the latest stable LTS at implementation
  time (probably 6.12.x).
- Exact tools-tree package list — refine by attempting a build and adding what
  kbuild reports missing.
- `tools_tree_digest` derivation — sha256 over `mkosi/kernel-builder/mkosi.conf`
  is the simple option; if the tools tree pulls in a `mkosi.tools.manifest`,
  hash that too.
- nspawn invocation: bind-mount layout, flags (likely `--bind=`, `--ephemeral`,
  `--register=no`), how to get env vars in (`--setenv=`).
- `merge_config.sh` lives at `scripts/kconfig/merge_config.sh` inside the
  extracted tree; verify in current kernel.

---

## Migration

For someone with an existing checkout pulling these changes:

1. New files (`kernel/version`, `kernel/required.config`, `kernel/hardening.config`,
   `kernel/config-x86_64.snapshot`, `mkosi/kernel-builder/mkosi.conf`) are committed.
2. First `steep build` after the change runs the kernel pipeline (slow — full kernel
   compile). Subsequent builds hit the cache.
3. `output/kernel/` is git-ignored; users re-create it on first build.
4. Existing `output/<name>/` directories from before this change become stale
   — their `manifest.json` lacks the `kernel` input block. Not auto-migrated;
   re-run `steep build` for any image you want a kernel-aware manifest for.

---

## Summary of file changes

**New:**

- `kernel/version`
- `kernel/required.config`
- `kernel/hardening.config`
- `kernel/config-x86_64.snapshot`
- `mkosi/kernel-builder/mkosi.conf`
- `src/kernel_cache.rs`
- `tests/kernel.rs`
- `tests/fixtures/kernel_manifest.json`

**Modified:**

- `src/lib.rs` — `KernelArgs` reshape
- `src/commands/kernel.rs` — full implementation replacing TODO stub
- `src/commands/build.rs` — Phase 0 wiring, console drop-in path, step counters
- `src/qemu.rs` — virtio-console wiring across tiers
- `mkosi/base/mkosi.conf` — drop `linux-generic`, drop module-init bundling,
  switch console to hvc0
- `mkosi/initrd/mkosi.conf` — drop `kmod`
- `mkosi/initrd/mkosi.extra/init` — drop module-loading block
- `tests/qemu.rs` — virtio-console assertions
- `tests/e2e.sh` — hvc0 scraping
- `.gitignore` — add `output/kernel/`,
  `mkosi/base/mkosi.extra/usr/lib/modules/`

**Follow-up (not blocking this design):**

- `README.md` — QEMU example, console wiring
- `docs/CONCEPTS.md` — sections on kernel modules and initrd contents are stale
