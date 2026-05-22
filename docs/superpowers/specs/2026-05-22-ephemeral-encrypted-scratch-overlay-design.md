# Ephemeral encrypted scratch overlay for SNP CVMs

## Problem

A steep-built image boots with a read-only dm-verity root (erofs) as the
overlay **lower** layer; all writes land on the overlay **upper** layer. When
no data disk is attached, the upper layer is a 2G RAM-backed tmpfs
(`mkosi/initrd/mkosi.extra/init`). A build task running inside the CVM that
needs more than a few GB of scratch space therefore runs out of room: it is
capped at 2G of RAM.

The goal is to give a running CVM a larger, **ephemeral** writable area that
grows the root filesystem transparently (no application or path awareness),
while preserving steep's confidentiality guarantees on untrusted SNP hosts.

## Scope

- **Target environment:** deployed SEV-SNP, where the host is untrusted.
- **Persistence:** ephemeral only. Nothing survives a reboot.
- **Transparency:** the entire writable root gains capacity; build tasks write
  wherever they normally would.

### Out of scope (v1)

- Persistent scratch.
- Integrity-protected scratch (authenticated encryption / dm-integrity).
- Simultaneous `LABEL=data` + `LABEL=scratch` disks on one VM.
- The existing unencrypted `LABEL=data` path's own confidentiality gap.

## Design

### Boot flow (`mkosi/initrd/mkosi.extra/init`)

After the dm-verity root is opened and mounted read-only as the lower layer,
the existing device scan over `/dev/vdb`, `/dev/vdc`, `/dev/vdd` is extended to
recognize a new label:

| Whole-device `blkid` LABEL | Upper layer |
|----------------------------|-------------|
| `scratch`                  | **new:** ephemeral encrypted disk |
| `data`                     | existing: persistent plaintext disk, bind-mounted at `/data` |
| neither                    | existing: 2G RAM tmpfs |

When a device with `LABEL=scratch` is found, the initrd:

1. Generates a random key in RAM only:
   `head -c 64 /dev/urandom > /run/scratch.key`
   (the initrd root is itself in SEV-SNP-encrypted RAM; the key is never
   written to any disk).
2. Opens a plain dm-crypt mapping over the **whole device**:
   `cryptsetup open --type plain --cipher aes-xts-plain64 --key-size 512 -d /run/scratch.key <dev> scratch`
3. Formats the mapped device fresh on every boot:
   `mkfs.ext4 -q /dev/mapper/scratch`
4. Mounts it and uses it as the overlay `upperdir`/`workdir`, exactly mirroring
   the existing data-disk overlay block (but without the `/data` bind-mount —
   scratch is not exposed as a named mount).

`--type plain` (not LUKS) is deliberate: there is no on-disk header to manage,
nothing persists, and the key is purely ephemeral. This is the canonical
ephemeral-encrypted-scratch pattern.

### Reboot behavior (documented, by design)

The `mkfs` in step 3 destroys the plaintext `LABEL=scratch` on first boot — the
raw device is now ciphertext. If the **same** device is reattached on a later
boot, `blkid` no longer reports `LABEL=scratch`, so the initrd falls back to the
2G tmpfs.

This is acceptable and not a loss of functionality: the boot-1 random key is
gone, so the prior contents are cryptographically unrecoverable regardless. The
only sane action on that device would be to reformat it. The deployment model
for ephemeral scratch is a **fresh disk per launch**, which cloud ephemeral
disks provide. Deployers who want scratch on every boot must attach a freshly
`LABEL=scratch`-labeled device each time.

### Detection precedence

`LABEL=scratch` is checked before `LABEL=data` in the scan. Mixing both on one
VM is out of scope; the first matching device wins per the scan order.

### Kernel config (`kernel/required.config`)

The kernel currently has `EXT4_FS` and `BLK_DEV_DM` but **not** dm-crypt or the
required ciphers. Add:

- `CONFIG_DM_CRYPT=y`
- `CONFIG_CRYPTO_XTS=y`
- `CONFIG_CRYPTO_AES=y`
- `CONFIG_CRYPTO_AES_NI_INTEL=y` (hardware AES for throughput)

The kernel is rebuilt and `kernel/config-x86_64.snapshot` regenerated as part of
the normal kernel build.

### initrd packages (`mkosi/initrd/mkosi.conf`)

- Add `e2fsprogs` (provides `mkfs.ext4`).
- `cryptsetup-bin` is already present.

### Launch side (`steep run` / `src/qemu.rs`)

Add `--scratch <SIZE>` to `steep run` (e.g. `--scratch 20G`). When present:

1. Create a sparse `scratch.raw` of the requested size in the output directory.
2. Give it a whole-device label the initrd will detect:
   `mkfs.ext4 -L scratch scratch.raw` (a whole-device ext4 with no partition
   table, so `blkid` reports `LABEL=scratch`). Requires `e2fsprogs` on the host
   running steep — already present wherever mkosi builds images.
3. Attach it as a **writable** second virtio drive (no `readonly=on`).

When absent, no scratch disk is attached and the VM uses the existing tmpfs
fallback. The disk is recreated fresh on each `steep run` invocation, matching
the fresh-disk-per-launch model.

This flag is primarily a local-testing/dev convenience: it is the mechanism that
makes the new initrd path exercisable in an integration test. In production on
real SNP hardware, the deployer attaches their own `LABEL=scratch` block device
at the same slot. Sizing lives entirely outside the guest — the initrd consumes
whatever block-device size it is handed.

## Security analysis

- **Confidentiality (preserved):** the key is drawn from the guest kernel RNG
  *after* the measured kernel/initrd have booted, lives only in SEV-SNP-encrypted
  RAM, and is never written to any disk. The untrusted host sees only ciphertext.
- **Integrity (not provided, out of scope):** plain dm-crypt gives
  confidentiality but not authentication — a malicious host can tamper with
  ciphertext blocks (corrupting, not reading, scratch data). This matches
  steep's existing trust model: the writable overlay is already unattested by
  design (see `docs/CONCEPTS.md`). Authenticated encryption (dm-integrity) is a
  possible future enhancement; it needs `CONFIG_DM_INTEGRITY` and adds overhead.
- **Attestation of the policy:** gating on `LABEL=scratch` (host-supplied disk
  state) rather than a measured kernel cmdline means the "this VM consumes a
  scratch disk" decision is not reflected in the launch measurement. This is
  acceptable and consistent with the existing `LABEL=data` mechanism: the disk is
  encrypted with an in-guest key and `mkfs`'d empty each boot, so the host cannot
  inject any content into the attested-empty upper layer — it can only deny or
  tamper, which an untrusted host can always do.

## Testing

- **Unit:** `steep run` argument parsing for `--scratch`; `qemu.rs` argument
  construction includes the writable second virtio drive only when `--scratch`
  is set, and never marks it `readonly=on`.
- **Integration:** boot a steep VM with `--scratch <SIZE larger than 2G>`;
  assert `df` on `/` reflects the scratch capacity rather than 2G; write more
  than 2G; reboot; confirm the upper layer is empty and the device re-encrypts
  with a different key (ciphertext differs).
- **Regression:** with no `--scratch` / no `LABEL=scratch` device, behavior is
  the unchanged 2G tmpfs; a `LABEL=data` disk still takes the existing
  persistent path and is still bind-mounted at `/data`.

## Files touched

- `kernel/required.config` — enable dm-crypt + AES-XTS.
- `kernel/config-x86_64.snapshot` — regenerated by the kernel build.
- `mkosi/initrd/mkosi.conf` — add `e2fsprogs`.
- `mkosi/initrd/mkosi.extra/init` — add the `LABEL=scratch` branch.
- `src/lib.rs` — add `--scratch` to `RunArgs`.
- `src/commands/run.rs` — create + label + attach the scratch disk.
- `src/qemu.rs` — thread the scratch disk into the writable virtio drive args.
- `README.md` — document `steep run --scratch` and the `LABEL=scratch` contract.
