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
oras pull ghcr.io/confidential-dot-ai/steep/base:latest
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
git clone https://github.com/confidential-dot-ai/steep.git
cd steep
bin/setup
bin/steep --help # builds steep using cargo and then runs it
```

The build host needs to be a real Linux system with `sudo` and the kernel/userns capabilities to run mkosi's sandbox. Most rootless dev containers can't — their nested user namespace can't `chown` to arbitrary uids/gids during package extraction, which mkosi requires.

### Developing

`bin/test` runs the test suite (`cargo nextest run`); `bin/lint` runs clippy
over all targets. Both are what CI runs, so a clean local pass should mean a
green PR.

## Scope

Steep builds **confidential VM images for AMD SEV-SNP and Intel TDX** —
measurable, dm-verity-protected, attestable VM images that boot inside an L0
hypervisor. The same UKI + disk artifacts boot under both TEEs; the manifest
records the platform-specific reference measurements alongside each other:

| Platform | Measurement registers in the manifest | Firmware |
|----------|---------------------------------------|----------|
| **SEV-SNP** | one entry per `--smp` in `snp_variants[]`: launch digest, IGVM file, page/VMSA counts | steep's IGVM-aware edk2 fork (default `output/OVMF.fd`) |
| **Intel TDX** | singleton `tdx`: `mrtd`, `rtmr1`, `rtmr2` (RTMR[0] floats by design — see "Trusted DSDT" below) | upstream OVMF with TDVF (default `/usr/share/ovmf/OVMF.fd`) |

Steep is **not** a builder for host/hypervisor images that themselves run
other VMs: the guest-oriented kernel, dm-verity initrd, and trusted-DSDT
override are all designed for the guest side of the trust boundary. For a
plain host or hypervisor image, use a general-purpose image builder such as
mkosi directly.

## Usage


### `steep build` — build a VM image

Produces `output/<name>/{disk.raw, uki.efi, manifest.json, roothash}` and (unless `--platform tdx`/`--skip-igvm`) one `guest-smp<N>.igvm` per `--smp` value.

```bash
steep build [OPTIONS] [NAME]
```

| Arg / flag | Default | Purpose |
|---|---|---|
| `NAME` | `base` | Subdirectory under `output/` for build artifacts. |
| `-c, --cloud-init <PATH>` | (none) | NoCloud `user-data` file baked into the verity root at `/var/lib/cloud/seed/nocloud/user-data`. Measured into the image. Standard cloud-init `#cloud-config` YAML — see [`examples/caddy.yaml`](examples/caddy.yaml) for a working example that serves a web page from the VM. |
| `-e, --extra <DIR>` | (none) | Directory whose contents are recursively copied **on top of** mkosi's base image filesystem. File modes and symlinks are preserved. Use this to bake binaries, systemd units, configuration files, etc. into the verity root. Measured. |
| `-p, --package <PKG>` | (none) | Extra apt package to install in the base image. Repeatable, also accepts comma-separated lists (`-p curl,jq,iproute2` or `-p curl -p jq`). Passed through to mkosi as `--package=`. |
| `--kernel-config-fragment <PATH>` | (none) | Extra kernel config fragment (kconfig `merge_config.sh` format) merged after `required.config` + `hardening.config`. Omitted → steep's hardened required+hardening baseline kernel. Lets a project enable extra kernel symbols without modifying steep. The build rewrites `kernel/config-x86_64.snapshot` with the resolved config (see [Snapshots](#snapshots)). |
| `--kernel-builder-package <PKG>` | (none) | Extra package installed into the kernel-builder tools tree (where the custom kernel is compiled), not the guest image. Repeatable and comma-separated. Use for build-time tools a fragment needs — e.g. `dwarves` (pahole) when the fragment enables `CONFIG_DEBUG_INFO_BTF`. |
| `-s, --script <FILE>` | (none) | mkosi post-install script (`--postinst-script`) run inside the image build with network enabled, so it can download resources. Measured — the script's effects land in the verity root. |
| `--profile dev` | off | Enable the `dev` profile: a systemd drop-in that gives root a passwordless autologin on the serial gettys, plus `console=ttyS0` on the measured cmdline. Useful for testing; changes the image measurement. Pair with `--kernel-config-fragment kernel/dev.config` to actually get ttyS0 output. **Don't ship with this on** — under the SNP threat model the host controls the serial port. |
| `--platform <snp\|tdx\|both>` | `both` | Which confidential-VM platform(s) to measure for. `both` emits both `snp_variants[]` IGVM measurements AND a singleton `tdx` measurement block. `snp` is IGVM-only. `tdx` skips IGVM and only computes the TDX registers. The same UKI + disk artifacts feed both measurement paths. |
| `--skip-igvm` | off | DEPRECATED — accepted as an alias for `--platform tdx` so older shell wrappers keep working. The combination `--skip-igvm --platform snp` is rejected (it asks for an SNP launch digest while also opting out of IGVM generation). |
| `--firmware <PATH>` | `output/OVMF.fd` (env: `STEEP_FIRMWARE`) | OVMF firmware binary used for SNP launch. Must be steep's edk2 build with the `IgvmHobArea` region (region type 0x200) — IGVM construction injects UKI/shim/cert bytes into that area. Ubuntu's stock OVMF does not have this region and will fail IGVM build. |
| `--tdx-firmware <PATH>` | `/usr/share/ovmf/OVMF.fd` (env: `STEEP_TDX_FIRMWARE`) | OVMF firmware used for TDX measurement. Must be a build with TDVF code paths compiled in (the `ovmf` package binary works). Steep's IGVM-aware firmware does NOT include TDVF — a TDX guest booted on it hangs silently in firmware. The TDX `mrtd` in the manifest is the hash of THIS firmware, not `--firmware`. Ignored when `--platform snp`. |
| `--memory <SIZE>` | `4G` | VM memory recorded in `manifest.json` (`build.memory`). `steep run` reads this when booting the image; not used at build time. QEMU-style suffix (`512M`, `8G`, `64G`). |
| `--smp <N>...` | `2 4 8 16` | vCPU counts to build IGVM variants for. Repeatable/space-separated. Each count produces a `guest-smp<N>.igvm` and an `snp_variants[]` manifest entry (SMP count is part of the SNP launch measurement). Recorded in `manifest.json`; `steep run` picks a variant from it. |

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
    --profile dev --skip-igvm

# With a custom kernel config fragment and an IGVM measurement
steep build myimage \
    --extra ./myimage/extra \
    --kernel-config-fragment ./myimage/kernel.config \
    --memory 8G --smp 4 \
    --firmware output/OVMF.fd
```

### `steep run` — boot a built VM in QEMU

```bash
steep run [OPTIONS] [DIR]
```

| Arg / flag | Default | Purpose |
|---|---|---|
| `DIR` | `output/base` | Output directory from `steep build` (contains `manifest.json`). |
| `--scratch <SIZE>` | (none) | Attach a fresh ephemeral disk labeled `scratch`; the initrd encrypts it with a random key and mounts it as expanded writable space. See [Ephemeral scratch space](#ephemeral-scratch-space). |
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
| `--registry <URL>` | `docker.io/confidentialai` | OCI Registry root. |
| `--name <NAME>` (push) | `<DIR basename>` | Image name segment. For `pull` the name is the positional `<NAME>` argument, not a flag. |
| `--cdi` (push) | off | Pack everything into a single `tar+gzip` layer with `disk.raw` under `disk/` — the layout KubeVirt CDI's registry importer expects. |
| `--tag <TAG>` | `latest` | Image tag. |

### `steep igvm` — generate additional IGVM SMP variants

`steep build` already emits one IGVM per `--smp` value. If you later need additional SMP counts for an existing build (each producing a distinct launch digest, since SMP is part of the measurement), `steep igvm` re-renders IGVMs without rebuilding:

```bash
steep igvm output/myimage --smp 1 2 4 8 --firmware output/OVMF.fd
```

## Kernel fragments

Steep ships a hardened guest kernel built from `kernel/version` (linux 6.16.12) with two **always-applied** fragments, plus an optional caller-supplied one.

| Fragment | What it adds | Applied |
|---|---|---|
| `kernel/required.config` | Filesystems, dm-verity, SEV-SNP guest support, devtmpfs | Always |
| `kernel/hardening.config` | Lockdown LSM, KASLR, stack protector, attack-surface trims (USB / PCI hotplug / DRM off, etc.) | Always |
| `--kernel-config-fragment <PATH>` | Whatever the caller's fragment enables — steep ships none | Only when the flag is passed |

steep itself builds only `required + hardening` — a minimal hardened microVM kernel, and **steep carries no project-specific kernel config**. A project that needs extra kernel symbols (a wider networking stack, additional filesystems, cgroup features, …) keeps its own fragment file in its own repo and passes it via `--kernel-config-fragment`. steep merges it after `required + hardening`; nothing else about the build changes.

### Trusted DSDT (TDX BadAML mitigation)

A TDX guest's firmware-supplied DSDT (Differentiated System Description Table)
contains AML bytecode that the guest kernel executes at kernel privilege during
ACPI init. Because the DSDT comes from the VMM in the TDX threat model,
arbitrary AML in the DSDT is an attack vector — the "BadAML" class.

Steep ships a minimal, audited DSDT (`mkosi/base/acpi-tables/dsdt.asl`) in
the initrd's early-cpio segment at `kernel/firmware/acpi/dsdt.aml`. The
kernel's `CONFIG_ACPI_TABLE_UPGRADE` feature scans the initrd for this path
at boot and **overrides** the VMM-supplied DSDT (replaces FADT's DSDT
pointer to point at the trusted bytes). The OEM ID, OEM Table ID, and
OEM Revision in the ASL are chosen to match QEMU's emission exactly so the
override condition in Linux's `acpi_table_initrd_override` actually fires —
a single trailing-byte mismatch falls through to the install-only path and
leaves the VMM's DSDT live. The runtime override is verifiable via
`dmesg | grep "Table Upgrade: override"` (the `override` keyword is the
load-bearing signal — `install` alone is a no-op).

The trusted DSDT bytes are part of the initrd, which is part of the UKI
and the IGVM file, so the override is itself attested:
  - on TDX, via the UKI sections hash in RTMR[2]
  - on SNP, via the IGVM launch digest

This is why steep's TDX manifest pins only MRTD + RTMR[1] + RTMR[2] and
leaves RTMR[0] unpinned: the VMM still drives RTMR[0] (TD HOB + remaining
ACPI tables that vary with memory size and SMP topology), but the
*executable* AML the kernel runs is the trusted one. Memory and SMP can
vary at deployment time without invalidating the manifest's TDX reference
values.

The kernel fragment `kernel/confidential.config` re-enables
`CONFIG_ACPI_TABLE_UPGRADE` (which the standard-threat-model hardening
fragment disables) and adds `CONFIG_INTEL_TDX_GUEST` + `CONFIG_TDX_GUEST_DRIVER`
+ `CONFIG_X86_X2APIC` (the last being a required dependency for the TDX
guest support). It's merged after `required.config` and `hardening.config`
so the last-wins semantics deliberately invert the hardening choices that
the trusted-DSDT design makes unnecessary.

### Snapshots

Each fragment combination resolves (via `make olddefconfig`) to a complete `.config`. steep writes that resolved config to `kernel/config-x86_64.snapshot`, a **lockfile** committed to git: every kernel build rewrites it automatically, and `git diff` is how you see what changed. A build never fails on snapshot drift.

After a build, review the snapshot:
- An **expected** change — you edited a fragment or bumped the kernel version — gets committed alongside that edit.
- An **unexpected** change is worth investigating: a kernel version bump can silently enable/disable cascading dependencies, and build-environment differences (mkosi version, toolchain version) between developers can shift the resolved config.

The snapshot reflects the most recent build's inputs. Building with `--kernel-config-fragment` resolves a different `.config` than steep's bare baseline, so the snapshot will show that fragment's effect; revert it with `git checkout kernel/config-x86_64.snapshot` if that build was a one-off.

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
├── OVMF.tdx.fd      TDX firmware (copy of the --tdx-firmware input; present for
│                    --platform tdx/both — its hash is the manifest's TDX `mrtd`)
└── guest-smp<N>.igvm  one IGVM per --smp value (absent when --platform tdx / --skip-igvm)
```

The manifest is the authoritative description of what's in the build. To verify an image you got from elsewhere, compare `manifest.json`'s `outputs.uki.sha256`, the `snp_variants[]` entry matching your vCPU count (`snp_variants[].measurement.snp_launch_digest`), and/or the `tdx` measurement block against the published expected values for the build inputs you trust.


Steep uses `mkosi` to build base image for Ubuntu 26.04 (Resolute Raccoon).
Built images is fully measured (see [Measurement Chain](#measurement-chain)),
and suitable for booting trusted confidential VMs.

Pass `--profile dev` to enable a passwordless root autologin on the serial console,
so `steep run` pops a shell. This changes the image measurement and must not be
used for production images — under the SNP threat model the host controls the
serial port.

## License

Steep is licensed under the [GNU Affero General Public License v3.0](LICENSE).
