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
  -machine q35 \
  -enable-kvm \
  -drive "if=pflash,format=raw,readonly=on,file=OVMF.fd" \
  -kernel uki.efi \
  -drive "file=disk.raw,format=raw,if=virtio" \
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

## Cloud-Init

The cloud-init YAML is injected as a static file into the NoCloud seed directory inside the verity root. It is measured (included in the dm-verity hash) but not executed at build time. Cloud-init runs normally when the VM boots.

Properties:
- Reproducible. The YAML file is static content with clamped timestamps.
- All modules work. Cloud-init runs in a fully booted system with systemd, networking, users, and services.
- Attestation proves "this VM was configured to do X." The config is attested, not the execution result.

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
