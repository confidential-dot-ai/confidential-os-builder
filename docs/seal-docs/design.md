# Design

## Disk Layout

`steep seal` produces a GPT disk with three partitions.

```
Partition 1 (ESP)          EFI boot files, FAT32
Partition 2 (root-data)    ext4 root filesystem, read-only
Partition 3 (root-verity)  dm-verity hash tree for partition 2
```

The disk is attached to QEMU via virtio (`-drive if=virtio`), appearing as `/dev/vda` inside the guest. Partition 2 is `/dev/vda2` (data), partition 3 is `/dev/vda3` (hashes).

## Boot Sequence

1. QEMU loads IGVM (or UKI directly in KVM/emulated mode)
2. OVMF firmware starts, finds UKI, executes it
3. Kernel starts with initrd in RAM
4. `/init` runs
   - `depmod -a` then `modprobe dm-verity overlay`
   - Parse `roothash=` from `/proc/cmdline` (glob protection via `set -f`)
   - Normalize roothash to lowercase, validate 64/96/128 hex chars
   - Wait for `/dev/vda2` (up to 30s)
   - `veritysetup open /dev/vda2 root /dev/vda3 $ROOTHASH`
   - Mount `/dev/mapper/root` read-only at `/sysroot-lower`
   - Mount tmpfs overlay at `/sysroot` (nosuid,nodev on upper layer)
   - Generate machine-id from `/proc/sys/kernel/random/uuid`
   - `exec switch_root /sysroot /sbin/init`
5. systemd starts from verified root
6. cloud-init runs (if configured)

## Overlay Model

dm-verity makes the root filesystem read-only. The overlay provides writability.

**Lower layer** is `/dev/mapper/root` mounted read-only. Every block is verified by dm-verity on read. **Upper layer** is tmpfs in RAM. All writes (new files, modifications, deletes) land here. **Merged view** is `/sysroot`, where reads check upper first, then fall through to verified lower.

Modifying an existing file copies it from lower to upper, then modifies the upper copy. The lower original remains untouched and verified. Deletes create whiteout files in the upper layer; the lower file still exists on disk but is hidden from the merged view.

On reboot the tmpfs upper layer is gone. The system starts fresh from the verified lower layer every time. Nothing persists across reboots unless a separate data disk is attached.

The upper layer is mounted with `nosuid,nodev`. Setuid/setgid bits are ignored on upper-layer binaries, preventing privilege escalation via planted setuid binaries. Device node creation is blocked, preventing crafted device nodes from accessing host devices. Legitimate setuid binaries (sudo, passwd) on the verified lower layer are unaffected.

## Two Build Targets

Steep uses mkosi twice.

**Initrd** (`mkosi/initrd/`) is a minimal cpio archive (~50MB). It contains veritysetup, kmod, bash, and the custom `/init` script. It exists only during the first seconds of boot.

**Base image** (`mkosi/base/`) is the full Ubuntu disk image with three partitions. It contains the OS, cloud-init, networking, and all packages. This is the root filesystem for the VM's lifetime.

The initrd is built first, then passed to the base image build so mkosi can bundle it into the UKI.

## QEMU Tiers

`steep run` detects hardware capabilities and selects the best launch mode.

| Tier | Requirements | Launch mode |
|------|-------------|-------------|
| SevSnp | QEMU with sev-snp-guest + igvm-cfg, /dev/kvm | IGVM measured boot, EPYC-Genoa CPU, memfd backend, sudo for /dev/sev |
| Kvm | /dev/kvm available | Direct UKI boot with KVM acceleration, OVMF firmware |
| Emulated | QEMU only | Software emulation, same args as Kvm without -enable-kvm |

---

## Threat Model

In the AMD SEV-SNP model, the host/hypervisor is adversarial. The cloud provider has physical access to the machine, controls QEMU, and can inspect/modify memory outside the TEE boundary. The attestation chain proves to a remote verifier that the VM is running exactly the expected software, despite the hostile host.

### What attestation proves

The SNP launch digest is a hardware-signed measurement of the initial VM state.

| Component | Measured | How |
|-----------|----------|-----|
| OVMF firmware | Yes | Pages loaded into IGVM |
| Linux kernel (vmlinuz) | Yes | Embedded in UKI, hashed into IGVM |
| Initrd | Yes | Embedded in UKI, hashed into IGVM |
| Kernel cmdline (including roothash) | Yes | Embedded in UKI, hashed into IGVM |
| Root filesystem content | Yes | Via dm-verity roothash in cmdline |
| Cloud-init config (boot-time mode) | Yes | Static file in verity root |
| Cloud-init execution results | No | Land on tmpfs overlay, unattested |
| Runtime modifications | No | Land on tmpfs overlay, unattested |

Attestation proves "this VM was told to do X at build time." It does not prove "X happened correctly at runtime" or "the system hasn't been modified since boot."

### Out of scope

**Runtime integrity** (proving the system is still running what was measured) requires IMA/EVM or runtime attestation, orthogonal to steep. **Data disk integrity** is not covered; only the root filesystem is protected by dm-verity. **Network security** is user-configured via cloud-init; steep does not inject default network policy.

### Machine-ID and host-provided randomness

The initrd generates machine-id from `/proc/sys/kernel/random/uuid`, seeded by the hypervisor's vRNG. In a strict TEE threat model, host-provided randomness is untrusted. However, the machine-id is on the overlay (not in the verity root) so it doesn't affect measurement, and it's used for systemd journal deduplication rather than security-critical operations. If stronger guarantees are needed, derive machine-id from attestation-bound material.

---

## Security Hardening

### Init script

The custom `/init` in the initrd includes several defensive measures.

- `set -f` before parsing `/proc/cmdline` prevents shell glob expansion of kernel parameters
- Roothash is normalized to lowercase and validated as 64/96/128 hex characters before passing to `veritysetup`
- Module loading uses `depmod -a` + `modprobe` instead of manual `insmod` to handle dependencies
- Every `sysrq-trigger` reboot is followed by `exit 1` in case sysrq is disabled
- `switch_root` from util-linux replaces manual chroot pivot

### QEMU argument injection

QEMU uses comma-delimited key=value pairs in `-object`, `-drive`, and similar arguments. A comma in a file path would inject additional properties.

```
-drive file=/path/with,inject=malicious,format=raw
```

Steep rejects paths containing commas before QEMU argument interpolation. All paths that appear in comma-delimited QEMU args are validated (disk, igvm, uki, firmware). Additionally, disk format is restricted to an allowlist (`raw`, `qcow2`), memory format is validated (digits + optional K/M/G/T suffix), and manifest fields are validated before constructing QEMU args.

### Serialization

All manifest structs use `#[serde(deny_unknown_fields)]` to reject unexpected fields during deserialization. This prevents injection of fields that might be interpreted by future code.

### Serial console / autologin

In the SNP threat model, the host controls the serial port. Autologin on ttyS0 gives the host an authenticated root session. Autologin is disabled by default and only injected when `--debug` is passed to `steep seal`. The debug autologin drop-in is removed via RAII cleanup before the image is finalized.
