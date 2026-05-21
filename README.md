# 🍵 steep, secure VM image builder

Steep is a confidential VM image builder for AMD SEV-SNP. It produces
bit-identical, attestable disk images from declarative configuration. The `build`
command builds a dm-verity protected root filesystem, bundles it into a Unified
Kernel Image (UKI), and optionally wraps it in an IGVM for measured launch on
SNP hardware.

| Document | What it covers |
|----------|---------------|
| [Concepts](docs/CONCEPTS.md) | Architecture, boot steps, image structure to ensure security |
| [Reproducibility](docs/REPRODUCIBILITY.md) | Changes needed, prior art, comparisons to other work |

## Running steep-built VMs

You can use the base images built by `steep` without installing the tool.

```
mkdir steep-base; cd steep-base
oras pull ghcr.io/lunal-dev/steep/base:latest
qemu-system-x86_64 \
  -machine q35 \
  -kernel uki.efi \
  -drive if=pflash,format=raw,readonly=on,file=OVMF.fd \
  -drive file=disk.raw,format=raw,if=virtio \
  -smp 1 -m 4G -nographic
```

## Installation

Steep runs on Ubuntu Linux. Clone the steep repo and run `bin/setup` to install everything you'll need (mkosi v26, qemu utils, swtpm, rust, cargo-nextest).

```bash
git clone https://github.com/confidential-ai/steep.git
cd steep
bin/setup
bin/steep --help # builds steep using cargo and then runs it
```

The build host needs to be a real Linux system with `sudo` and the kernel/userns capabilities to run mkosi's sandbox. Most rootless dev containers can't — their nested user namespace can't `chown` to arbitrary uids/gids during package extraction, which mkosi requires.

## Scope

Steep builds **SEV-SNP guest images** — measurable, dm-verity-protected, attestable VM images that boot inside an L0 hypervisor. It is **not** a builder for host/hypervisor images that themselves run other VMs: steep's guest-oriented kernel, IGVM measurement, and verity initrd are all designed for the guest side of the trust boundary. For a plain host or hypervisor image, use a general-purpose image builder such as mkosi directly.

## Usage


### `steep build` — build a VM image

Produces `output/<name>/{disk.raw, uki.efi, manifest.json, roothash}` and optionally `guest.igvm`.

```bash
steep build [OPTIONS] [NAME]
```

| Arg / flag | Default | Purpose |
|---|---|---|
| `NAME` | `base` | Subdirectory under `output/` for build artifacts. |
| `-c, --cloud-init <PATH>` | (none) | NoCloud `user-data` file baked into the verity root at `/var/lib/cloud/seed/nocloud/user-data`. Measured into the image. |
| `-e, --extra <DIR>` | (none) | Directory whose contents are recursively copied **on top of** mkosi's base image filesystem. File modes and symlinks are preserved. Use this to bake binaries, systemd units, configuration files, etc. into the verity root. Measured. |
| `-p, --package <PKG>` | (none) | Extra apt package to install in the base image. Repeatable, also accepts comma-separated lists (`-p curl,jq,iproute2` or `-p curl -p jq`). Passed through to mkosi as `--package=`. |
| `--kernel-config-fragment <PATH>` | (none) | Extra kernel config fragment (kconfig `merge_config.sh` format) merged after `required.config` + `hardening.config`. Omitted → steep's hardened required+hardening baseline kernel. Lets a project enable extra kernel symbols without modifying steep. The build rewrites `kernel/config-x86_64.snapshot` with the resolved config (see [Snapshots](#snapshots)). |
| `--console` | off | Inject a systemd drop-in that gives root a passwordless autologin on `hvc0`. Useful for testing; changes the image measurement. **Don't ship with this on** — under the SNP threat model the host controls the serial port. |
| `--skip-igvm` | off | Don't generate `guest.igvm`. The resulting `disk.raw` + `uki.efi` still boot under KVM (or directly on UEFI) — you just don't get the SNP launch digest that proves what the firmware loaded. Useful when the image is meant to run as an L1 guest of a cloud that does its own SEV-SNP attestation, or for local QEMU testing. |
| `--firmware <PATH>` | `output/OVMF.fd` (env: `STEEP_FIRMWARE`) | OVMF firmware binary that's bundled into the IGVM. Required unless `--skip-igvm`. |
| `--memory <SIZE>` | `4G` | VM memory recorded in `manifest.json` (`build.memory`). `steep run` reads this when booting the image; not used at build time. QEMU-style suffix (`512M`, `8G`, `64G`). |
| `--smp <N>` | `2` | vCPU count recorded in `manifest.json` (`build.smp`), used by `steep run` and (when generating IGVM) by the SNP launch measurement computation. |

#### Examples

```bash
# Default base image, KVM-bootable, no measurement
steep build --skip-igvm

# A larger image: bake in extra files + packages, more RAM, debug console
steep build myimage \
    --extra ./myimage/extra \
    --cloud-init ./myimage/user-data \
    --package curl,jq \
    --memory 8G --smp 4 \
    --console --skip-igvm

# With a custom kernel config fragment and an IGVM measurement
steep build myimage \
    --extra ./myimage/extra \
    --kernel-config-fragment ./myimage/kernel.config \
    --memory 8G --smp 4 \
    --firmware output/OVMF.fd
```

### `steep run` — boot a built VM in QEMU

```bash
steep run [OPTIONS] <DIR>
```

| Arg / flag | Default | Purpose |
|---|---|---|
| `DIR` | required | Output directory from `steep build` (contains `manifest.json`). |
| `--port-forward HOST:GUEST` | (none) | Forward a host port to a guest port. Repeatable: `--port-forward 8080:80 --port-forward 2222:22`. |
| `--qemu-bin <PATH>` | `qemu-system-x86_64` (env: `STEEP_QEMU_BIN`) | QEMU binary to invoke. |
| `--firmware <PATH>` | (manifest, or arg) (env: `STEEP_FIRMWARE`) | OVMF firmware override. Needed when the image was built with `--skip-igvm` and you're booting on KVM (which needs the firmware separately rather than as part of an IGVM). |

`steep run` auto-detects the best available QEMU tier:
- **SEV-SNP** if QEMU has `sev-snp-guest` + `igvm-cfg` support and `/dev/kvm` is present. Uses the IGVM, reports the launch digest.
- **KVM** if `/dev/kvm` is present but SNP support is missing. Loads the UKI + OVMF directly. No measurement.
- **Emulated** otherwise. Same as KVM but in software. Very slow; useful for CI smoke tests only.

### `steep kernel` — (re)build the custom kernel

Usually called transparently by `steep build`. Run directly when you've edited a fragment or bumped the kernel version.

```bash
steep kernel [OPTIONS]
```

| Arg / flag | Default | Purpose |
|---|---|---|
| `-o, --output <DIR>` | `output/kernel` | Where the resulting `vmlinuz`, `manifest.json`, build cache live. |
| `--kernel-config-fragment <PATH>` | (none) | Extra config fragment merged after required + hardening. Omitted → steep's baseline kernel. |
| `-f, --force` | off | Bypass the kernel cache. Forces a full rebuild even if the manifest fingerprint matches. |

Every build rewrites `kernel/config-x86_64.snapshot` with the resolved `.config` (see [Snapshots](#snapshots)). Typical lifecycle when editing a fragment:
 
 ```bash
# 1. Edit a fragment — steep's own kernel/hardening.config, or a caller's.
$EDITOR kernel/hardening.config

# 2. Rebuild. Resolves the new .config and rewrites the snapshot (~10 min).
#    Add --kernel-config-fragment /path/to/kernel.config for a caller fragment.
sudo -E env "PATH=$PATH" ./target/release/steep kernel
 
# 3. Review the snapshot change and commit it alongside the fragment edit.
git diff kernel/config-x86_64.snapshot
```
 
### `steep push` / `steep pull` — OCI registry transfer

Uses `oras` (must be on PATH).

```bash
steep push [OPTIONS] <DIR>
steep pull [OPTIONS] <NAME>
```

| Flag | Default | Purpose |
|---|---|---|
| `--registry <URL>` | `ghcr.io/lunal-dev` | Registry root. |
| `--name <NAME>` | `<DIR basename>` (push) / required (pull) | Image name segment. |
| `--tag <TAG>` | `latest` | Image tag. |

### `steep igvm` — generate additional IGVM SMP variants

After `steep build` with `--smp N`, the manifest captures one IGVM. If you want to publish the same image with multiple SMP counts (each producing a distinct launch digest, since SMP is part of the measurement), `steep igvm` re-renders the IGVM for additional counts:

```bash
steep igvm output/myimage --smp 1 2 4 8 --firmware output/OVMF.fd
```

## Kernel fragments

Steep ships a hardened guest kernel built from `kernel/version` (linux 6.12.84) with two **always-applied** fragments, plus an optional caller-supplied one.

| Fragment | What it adds | Applied |
|---|---|---|
| `kernel/required.config` | Filesystems, dm-verity, SEV-SNP guest support, devtmpfs | Always |
| `kernel/hardening.config` | Lockdown LSM, KASLR, stack protector, attack-surface trims (USB / PCI hotplug / DRM off, etc.) | Always |
| `--kernel-config-fragment <PATH>` | Whatever the caller's fragment enables — steep ships none | Only when the flag is passed |

steep itself builds only `required + hardening` — a minimal hardened microVM kernel, and **steep carries no project-specific kernel config**. A project that needs extra kernel symbols (a wider networking stack, additional filesystems, cgroup features, …) keeps its own fragment file in its own repo and passes it via `--kernel-config-fragment`. steep merges it after `required + hardening`; nothing else about the build changes.

### Snapshots

Each fragment combination resolves (via `make olddefconfig`) to a complete `.config`. steep writes that resolved config to `kernel/config-x86_64.snapshot`, a **lockfile** committed to git: every kernel build rewrites it automatically, and `git diff` is how you see what changed. A build never fails on snapshot drift.

After a build, review the snapshot:
- An **expected** change — you edited a fragment or bumped the kernel version — gets committed alongside that edit.
- An **unexpected** change is worth investigating: a kernel version bump can silently enable/disable cascading dependencies, and build-environment differences (mkosi version, toolchain version) between developers can shift the resolved config.

The snapshot reflects the most recent build's inputs. Building with `--kernel-config-fragment` resolves a different `.config` than steep's bare baseline, so the snapshot will show that fragment's effect; revert it with `git checkout kernel/config-x86_64.snapshot` if that build was a one-off.

## Measurement Chain

The attestation model rests on a deterministic chain from source configuration to hardware-signed measurement.

```
cloud-init YAML            (-c flag)
    |
extra/ contents            (-e flag)
    |  copied into image filesystem
    v
erofs root filesystem
    |  dm-verity hash tree
    v
roothash (SHA-256)
    |  embedded in kernel cmdline
    v
UKI (kernel + initrd + cmdline as one EFI binary)
    |  bundled with OVMF firmware
    v
IGVM (measured by SNP hardware on launch)
    |
    v
SNP launch digest (hardware-signed, unforgeable)
```

Change one file in `--extra`, one byte of the cloud-init payload, the kernel cmdline, or the kernel binary — and the roothash changes, which changes the UKI, which changes the IGVM measurement. A remote verifier checks the launch digest against a published expected value and can trust the entire stack.

## Output Artifacts

```
output/<name>/
├── disk.raw         GPT disk image (ESP + erofs root + verity hash partitions)
├── uki.efi          Unified Kernel Image (kernel + initrd + cmdline)
├── roothash         SHA-256 hex string of the verity root
├── manifest.json    Build metadata: input hashes, output hashes, smp/memory,
│                    per-fragment shas, optional SNP measurement
├── OVMF.fd          Firmware (copy of the --firmware input; bundled here so the
│                    output dir is self-contained for `steep run` and `steep push`)
└── guest.igvm       IGVM file (absent when --skip-igvm)
```

The manifest is the authoritative description of what's in the build. To verify an image you got from elsewhere, compare `manifest.json`'s `outputs.uki.sha256` and (with IGVM) `measurement.snp_launch_digest` against the published expected values for the build inputs you trust.


Steep uses `mkosi` to build base image for Ubuntu 26.04 (Resolute Raccoon).
Built images is fully measured (see [Measurement Chain](#measurement-chain)),
and suitable for booting trusted confidential VMs.

Pass `--console` to enable a passwordless root autologin on the serial console,
so `steep run` pops a shell. This changes the image measurement and must not be
used for production images — under the SNP threat model the host controls the
serial port.
