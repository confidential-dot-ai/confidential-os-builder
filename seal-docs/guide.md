# Guide

## Prerequisites

```bash
bin/setup
```

This installs mkosi, igvm-tools (via cargo), and copies OVMF firmware to `~/.local/share/steep/OVMF.fd`.

Required tools:
- mkosi 27+ (in `~/.local/bin`)
- cargo / Rust toolchain
- QEMU with virtio support (SNP requires an IGVM-enabled QEMU build)
- OVMF firmware (system package or custom build)

## Building Steep

```bash
cargo build          # debug
cargo build --release
```

Do not wrap `steep` in `sudo` directly. Steep calls sudo where needed (mkosi, QEMU with /dev/sev).

---

## Seal Commands

### Boot-time cloud-init (production path)

```bash
steep seal --cloud-init config.yaml -o output/my-image \
  --firmware /path/to/OVMF.fd \
  --igvm-tools /path/to/igvm-tools
```

Produces `disk.raw`, `uki.efi`, `roothash`, `manifest.json`, `guest.igvm`.

### Skip IGVM (faster, for development)

```bash
steep seal --skip-igvm --cloud-init config.yaml -o output/my-image
```

Everything except `guest.igvm`. Platform is `generic` instead of `snp`.

### Bake mode

```bash
steep seal --cloud-init config.yaml --bake -o output/my-image
```

Executes cloud-init at build time in a chroot. See [Bake Mode](#bake-mode) below for limitations.

### Debug mode

```bash
steep seal --debug --cloud-init config.yaml -o output/my-image
```

Injects passwordless root autologin on ttyS0. The drop-in is removed after the build and does not persist unless `--debug` is specified.

### Bare image (no cloud-init)

```bash
steep seal --skip-igvm -o output/bare
```

---

## Running a Sealed Image

```bash
steep run output/my-image
```

Reads smp, memory, format, and platform from `manifest.json`. Detects QEMU capabilities (SNP > KVM > emulated) and launches accordingly.

### Manual QEMU launch (KVM)

```bash
qemu-system-x86_64 \
  -machine q35 -enable-kvm \
  -drive "if=pflash,format=raw,readonly=on,file=/path/to/OVMF.fd" \
  -kernel output/my-image/uki.efi \
  -drive "file=output/my-image/disk.raw,format=raw,if=virtio" \
  -smp 1 -m 4G -nographic \
  -netdev "user,id=net0,hostfwd=tcp::8080-:8080" \
  -device virtio-net-pci,netdev=net0
```

### Manual QEMU launch (SNP with IGVM)

```bash
sudo /path/to/qemu-system-x86_64 \
  -enable-kvm -cpu EPYC-Genoa \
  -machine q35,confidential-guest-support=sev0,igvm-cfg=igvm0,memory-backend=ram1,kernel-irqchip=split \
  -object igvm-cfg,id=igvm0,file=output/my-image/guest.igvm \
  -object memory-backend-memfd,id=ram1,size=4G,share=true \
  -object sev-snp-guest,id=sev0,cbitpos=51,reduced-phys-bits=1 \
  -drive file=output/my-image/disk.raw,format=raw,if=virtio \
  -smp 1 -m 4G -nographic -no-reboot \
  -serial mon:stdio -monitor none \
  -netdev user,id=net0,hostfwd=tcp::8080-:8080 \
  -device virtio-net-pci,netdev=net0
```

### Manifest inspection

```bash
python3 -m json.tool output/my-image/manifest.json
```

Fields include `version`, `build.platform` (generic or snp), `build.format`, `build.smp`, `build.memory`, SHA-256 hashes of inputs/outputs, and `measurement.snp_launch_digest` (SHA-384, only when platform is snp).

---

## Cloud-Init Modes

### Boot-time (default)

The cloud-init YAML is injected as a static file into the NoCloud seed directory inside the verity root. It is measured (included in the dm-verity hash) but not executed at build time. Cloud-init runs normally when the VM boots.

Properties:
- Reproducible. The YAML file is static content with clamped timestamps.
- All modules work. Cloud-init runs in a fully booted system with systemd, networking, users, and services.
- Attestation proves "this VM was configured to do X." The config is attested, not the execution result.

This is the production path and the reproducibility target.

### Bake Mode

Bake mode executes cloud-init at build time inside a chroot, so the results land in the verity root and are measured.

Properties:
- Not reproducible. Live apt fetches, compilation non-determinism, and cloud-init state vary between runs. Verified via artifact signing.
- Partial module support (see tables below).
- Attestation proves "this VM contains the exact results of running X."

**What works in bake mode**

| Module | Notes |
|--------|-------|
| `write_files` | Writes bytes to paths, no system dependencies. Most reliable bake module. |
| `packages` | apt works in chroot (network available, DNS set to 1.1.1.1/8.8.8.8) |
| `runcmd` | Arbitrary commands work if they don't depend on running services |
| `apt` (sources, repos, keys) | Operates on filesystem directly |

**What fails in bake mode**

| Module | Why |
|--------|-----|
| `users` / `groups` | useradd/groupadd fail without PAM/nsswitch in chroot |
| `ssh_authorized_keys` | Depends on the user existing first |
| `ssh_host_keys` | ssh-keygen fails without /dev/urandom in chroot |
| `locale` | locale-gen not available |
| `growpart` / `resizefs` | No real block devices in chroot |
| Any service-dependent module | No running systemd, dbus, or services |

**Rule of thumb.** Use `--bake` for pre-installing packages and writing config files. Use boot-time cloud-init for user setup, SSH keys, and service configuration.

**Bake failures are fatal.** If any cloud-init stage fails during bake, the build fails. A "successful" build that silently skipped user setup or SSH keys is worse than a failed build. The four stages run in order: `init --local`, `init`, `modules --mode=config`, `modules --mode=final`.

**Trust model.** `--bake` executes user-data as root on the build machine inside a chroot with bind-mounted `/dev`. The user-data is trusted. Network access is available. Bake mode is for operator-authored configs only. The real risk is runcmd execution as root, not the /dev bind-mount. A CLI warning is emitted when `--bake` is used. Proper sandboxing (systemd-nspawn, bubblewrap) is not implemented; revisit if bake becomes a production path.

**Heavy builds.** Cargo builds in chroot can exhaust system RAM and OOM-kill the process. Use `CARGO_BUILD_JOBS=1` or `-j2` in runcmd.

### Nftables / firewall

Firewall rules are user-controlled via cloud-init, not injected by steep.

```yaml
write_files:
  - path: /etc/nftables.conf
    content: |
      # your rules here
runcmd:
  - systemctl enable nftables
```

With `--bake`, the rules are sealed into the verity root and measured.

---

## Testing

### Unit tests

```bash
cargo test
```

33 tests covering QEMU argument construction, manifest serialization, IGVM invocation args, CLI structure, and tool resolution.

### E2E tests

```bash
cargo build
sudo ./tests/e2e.sh
```

The e2e test seals an image with boot-time cloud-init (write_files + HTTP server), validates output artifacts and manifest structure, optionally seals with IGVM and checks reproducibility, and optionally boots the VM and verifies cloud-init applied via HTTP health check.

Environment variables for IGVM/boot tests:

```bash
export STEEP_FIRMWARE=/path/to/OVMF.fd
export STEEP_IGVM_TOOLS=/path/to/igvm-tools
```
