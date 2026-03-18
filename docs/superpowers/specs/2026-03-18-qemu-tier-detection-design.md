# QEMU Tier Detection for `steep run`

## Problem

`steep run` hardcodes SEV-SNP + IGVM in its QEMU invocation. This means it fails on machines where QEMU was compiled without IGVM/SEV-SNP support, or where KVM is unavailable. The command should detect what's available and run the image with the best available tier.

## Tiers

| Tier | Requirements | QEMU flags |
|------|-------------|------------|
| **SevSnp** | `sev-snp-guest` + `igvm-cfg` in QEMU object types, `/dev/kvm` accessible | `-machine q35,confidential-guest-support=sev0,igvm-cfg=igvm0`, `-object sev-snp-guest,...`, `-object igvm-cfg,...` |
| **Kvm** | `/dev/kvm` accessible | `-machine q35`, `-enable-kvm`, `-drive if=pflash,...,file=<firmware>`, `-kernel <uki>` |
| **Emulated** | None | `-machine q35`, `-drive if=pflash,...,file=<firmware>`, `-kernel <uki>` |

## Detection

Probe QEMU capabilities at runtime before launching:

1. Verify `qemu-system-x86_64` is in PATH (via `tools::require`). Fail early with a clear error if absent.
2. Run `qemu-system-x86_64 -object help` and parse the output. Each line contains a QOM type name. Check whether `sev-snp-guest` and `igvm-cfg` each appear as a substring of any line. Matching is case-sensitive.
3. Check if `/dev/kvm` is accessible by attempting to open it with read-write (`OpenOptions::new().read(true).write(true).open("/dev/kvm")`). Read-write matches how QEMU itself opens the device (`O_RDWR` in `kvm-all.c`). `Path::exists()` is insufficient because it does not verify permissions.
4. Select tier:
   - Both QEMU object types present AND `/dev/kvm` accessible → **SevSnp**
   - `/dev/kvm` accessible → **Kvm**
   - Otherwise → **Emulated**

**Source for `-object help` behavior:** QEMU's `user_creatable_print_help()` in `qom/object_interfaces.c` calls `user_creatable_print_types()` when the type is `"help"`, which enumerates all QOM types via `object_class_get_list_sorted(TYPE_USER_CREATABLE, false)`.

## Warnings

Print a clear warning for non-SevSnp tiers before launching:

- **Kvm:** `"WARNING: QEMU lacks IGVM/SEV-SNP support. Running with KVM acceleration only — no confidential computing guarantees."`
- **Emulated:** `"WARNING: Neither SEV-SNP nor KVM available. Running in pure emulation mode — this will be slow."`

## Artifact Validation

Validation depends on the detected tier:

- **SevSnp:** Require `guest.igvm` exists in output directory (current behavior).
- **Kvm / Emulated:** Require `uki.efi` exists in the output directory (`args.dir.join("uki.efi")`). Also require that the firmware file at `manifest.inputs.firmware.path` exists (this is the OVMF binary used at build time; it must still be present at run time).

## UEFI Firmware for Kvm/Emulated Tiers

UKIs are PE/COFF binaries that require UEFI firmware to boot. In the SevSnp tier, OVMF is bundled inside the IGVM file. In Kvm/Emulated tiers, OVMF must be passed to QEMU explicitly via pflash:

```
-drive if=pflash,format=raw,readonly=on,file=<firmware>
```

The firmware path is read from `manifest.inputs.firmware.path`. If the file no longer exists at that path, `steep run` fails with a clear error explaining the firmware is missing.

The firmware must be a combined/single OVMF image (not a split `OVMF_CODE.fd` + `OVMF_VARS.fd` pair). This matches the existing contract — the build pipeline passes the same single firmware file into `igvm-tools`.

## Changes to `qemu.rs`

### New types

```rust
pub enum QemuTier {
    SevSnp,
    Kvm,
    Emulated,
}
```

### New function: `detect_tier()`

Returns `anyhow::Result<QemuTier>`. Calls `tools::require("qemu-system-x86_64")` first, then runs the probes described in the Detection section.

To keep the probing logic unit-testable, the actual tier selection is in a separate pure function:

```rust
pub fn select_tier(object_help_output: &str, kvm_available: bool) -> QemuTier
```

This function is `pub` so integration tests in `tests/qemu.rs` can exercise it. `detect_tier()` gathers the inputs (running QEMU, checking `/dev/kvm`) and delegates to `select_tier()`.

### Modified `QemuArgs`

```rust
pub struct QemuArgs {
    pub tier: QemuTier,
    pub igvm: Option<PathBuf>,      // Some only for SevSnp
    pub uki: Option<PathBuf>,       // Some for Kvm and Emulated
    pub firmware: Option<PathBuf>,  // Some for Kvm and Emulated (OVMF)
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
    pub port_forwards: Vec<(u16, u16)>,
}
```

### Modified `to_args()`

Branches on `self.tier`. Uses `.expect()` on the `Option` fields required by each tier — a `None` value is a programming error (the caller in `commands/run.rs` is responsible for setting the correct fields per tier), not a runtime condition.

- **SevSnp:** Current behavior (`-machine q35,confidential-guest-support=sev0,igvm-cfg=igvm0`, `-object sev-snp-guest,...`, `-object igvm-cfg,...`). Expects `igvm` to be `Some`.
- **Kvm:** `-machine q35`, `-enable-kvm`, `-drive if=pflash,format=raw,readonly=on,file=<firmware>`, `-kernel <uki>`. Expects `uki` and `firmware` to be `Some`.
- **Emulated:** `-machine q35`, `-drive if=pflash,format=raw,readonly=on,file=<firmware>`, `-kernel <uki>`. Expects `uki` and `firmware` to be `Some`.

All tiers share: `-drive` (disk), `-smp`, `-m`, `-nographic`, and port forward args.

### Modified `launch()`

Keep the existing `tools::require("qemu-system-x86_64")` call for defense-in-depth (it's cheap, and `launch()` could be called without `detect_tier()` in the future).

Update the `tracing::info!` call to log tier-appropriate fields:

- **SevSnp:** log `igvm` path (current behavior).
- **Kvm/Emulated:** log `uki` and `firmware` paths.

All tiers log `disk`, `smp`, `memory`.

## Changes to `commands/run.rs`

Updated flow:

1. Validate directory exists and read manifest (unchanged).
2. Call `qemu::detect_tier()`.
3. Print warning if tier is Kvm or Emulated.
4. Validate artifacts based on tier:
   - SevSnp: require `guest.igvm` exists in output directory.
   - Kvm/Emulated: require `uki.efi` exists in output directory. Require firmware exists at `manifest.inputs.firmware.path`.
5. Determine qemu disk format (unchanged).
6. Parse port forwards (unchanged).
7. Construct `QemuArgs` with detected tier and appropriate paths.
8. Launch QEMU.

## Test Changes (`tests/qemu.rs`)

- Update existing tests to construct `QemuArgs` with `tier: QemuTier::SevSnp`, `igvm: Some(...)`, `uki: None`, `firmware: None`.
- Add tests for **Kvm tier:** verify `-enable-kvm` present, `-kernel <uki>` present, pflash firmware present, no SEV-SNP/IGVM objects.
- Add tests for **Emulated tier:** verify no `-enable-kvm`, `-kernel <uki>` present, pflash firmware present, no SEV-SNP/IGVM objects.
- Add unit tests for **`select_tier()`**: test all three tier selections by passing synthetic `-object help` output strings and boolean KVM flags.
