# QEMU Tier Detection for `steep run`

## Problem

`steep run` hardcodes SEV-SNP + IGVM in its QEMU invocation. This means it fails on machines where QEMU was compiled without IGVM/SEV-SNP support, or where KVM is unavailable. The command should detect what's available and run the image with the best available tier.

## Tiers

| Tier | Requirements | QEMU flags |
|------|-------------|------------|
| **SevSnp** | `sev-snp-guest` + `igvm-cfg` in QEMU object types, `/dev/kvm` accessible | `-machine q35,confidential-guest-support=sev0,igvm-cfg=igvm0`, `-object sev-snp-guest,...`, `-object igvm-cfg,...` |
| **Kvm** | `/dev/kvm` accessible | `-machine q35`, `-enable-kvm`, `-kernel <uki>` |
| **Emulated** | None | `-machine q35`, `-kernel <uki>` |

## Detection

Probe QEMU capabilities at runtime before launching:

1. Run `qemu-system-x86_64 -object help` and parse the output (a list of user-creatable QOM types, one per line, under the heading "List of user creatable objects:"). Check for `sev-snp-guest` and `igvm-cfg`.
2. Check if `/dev/kvm` exists and is accessible.
3. Select tier:
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
- **Kvm / Emulated:** Require UKI file exists. The path is read from `manifest.outputs.uki.path`.

## Changes to `qemu.rs`

### New types

```rust
pub enum QemuTier {
    SevSnp,
    Kvm,
    Emulated,
}
```

### New function

`detect_tier()` → `anyhow::Result<QemuTier>`: runs the probes described above and returns the detected tier.

### Modified `QemuArgs`

```rust
pub struct QemuArgs {
    pub tier: QemuTier,
    pub igvm: Option<PathBuf>,  // Some only for SevSnp
    pub uki: Option<PathBuf>,   // Some for Kvm and Emulated
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
    pub port_forwards: Vec<(u16, u16)>,
}
```

### Modified `to_args()`

Branches on `self.tier`:

- **SevSnp:** Current behavior (confidential-guest-support, sev-snp-guest, igvm-cfg objects).
- **Kvm:** `-machine q35`, `-enable-kvm`, `-kernel <uki>`.
- **Emulated:** `-machine q35`, `-kernel <uki>`.

All tiers share: `-drive`, `-smp`, `-m`, `-nographic`, and port forward args.

## Changes to `commands/run.rs`

Updated flow:

1. Validate directory exists and read manifest (unchanged).
2. Call `qemu::detect_tier()`.
3. Print warning if tier is Kvm or Emulated.
4. Validate artifacts based on tier:
   - SevSnp: require `guest.igvm` exists.
   - Kvm/Emulated: require UKI file exists (path from `manifest.outputs.uki.path`).
5. Parse port forwards (unchanged).
6. Construct `QemuArgs` with detected tier and appropriate paths.
7. Launch QEMU.

## Test Changes (`tests/qemu.rs`)

- Update existing tests to construct `QemuArgs` with `tier: QemuTier::SevSnp`, `igvm: Some(...)`, `uki: None`.
- Add tests for **Kvm tier:** verify `-enable-kvm` present, `-kernel <uki>` present, no SEV-SNP/IGVM objects.
- Add tests for **Emulated tier:** verify no `-enable-kvm`, `-kernel <uki>` present, no SEV-SNP/IGVM objects.
- `detect_tier()` is not unit-tested (probes real system state).
