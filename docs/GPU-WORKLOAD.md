# GPU workload image

`gpu-workload` is an opt-in, operator-managed CVM environment for ordinary GPU
applications and containers. It generalizes the tested container-ready PoC
image without customer keys, models, IPs or GPU counts. It is distinct from
the hardened base and the serial-autologin `dev` profile.

## Contents and policy

- Docker, containerd and runc with NVIDIA CDI device injection.
- NVIDIA open driver `595.71.05` (from `gpu`), CUDA `13.2.2`, NCCL `2.29.7`,
  OpenMPI and checksum-pinned `nccl-tests`.
- Git, Python pip/venv, build tools and normal package-manager metadata.
- Key-only SSH, including the launch-bound `opkeydata` public-key bundle.
- GPU attestation (from `attest-gpu`) and GPU initialization before SSH.
- Docker bridge/host networking, DNS, forwarding and NAT. No guest egress
  allowlist or inbound firewall policy is installed. Host/provider networking
  and KubeVirt port exposure remain separate responsibilities. Authenticate
  and encrypt application traffic; prefer localhost-bound services plus SSH
  tunnels until an application access policy is configured.

The kernel fragment enables privileged cgroup BPF, user namespaces, veth,
bridges, netfilter, io_uring, userfaultfd, FUSE, TUN and CRIU interfaces. It
retains module signatures, the boot-time module-loading latch, kernel lockdown,
the trusted-AML policy, measured boot and dm-verity. Unprivileged BPF remains
disabled. These extra interfaces are an intentional compatibility tradeoff,
not the hardened base image's minimal attack surface.

**Runtime installs are not launch-measured.** `/boot`, `/etc`, `/opt` and `/usr`
have writable ephemeral overlays; software and configuration there can change
after attestation. The measured lower filesystem remains verity protected.
Attestation identifies that lower image and its launch, not current application
code, Docker layers, SSH configuration or installed packages. Guest root is
trusted to administer those. See [the threat model](THREAT_MODEL.md).

Without an encrypted scratch disk, all overlays share the initrd's 2 GiB RAM
backing store. Use `/var/lib/user-data` for large model caches, Docker data and
Python virtual environments. Ephemeral package installs do not survive reboot.
Additional unsigned/DKMS kernel modules still cannot be installed or loaded.

## Build and publish

On a Linux build host with `bin/setup` prerequisites, supply the private key
matching the committed public certificate as `MODULE_SIG_KEY` or
`MODULE_SIG_KEY_PEM`. Never commit or bake that private key.

```bash
bin/confos kernel --kernel-config-fragment kernel/gpu-workload.config
MODULE_SIG_CERT=kernel/module-signing.crt bin/confos-fetch-gpu
bin/confos build gpu-workload --platform tdx --memory 128G \
  --profile gpu --profile attest-gpu --profile ssh --profile gpu-workload \
  --kernel-config-fragment kernel/gpu-workload.config
```

Sync/post-install hooks fetch verified CUDA/NCCL inputs and automatically
compile MPI and non-MPI tests. The build memory setting is not a GPU-count
setting. TDX CPU, memory and device layouts still need hardware acceptance.

The `Build GPU Workload Image` workflow uses the large runner and caches the
kernel/tools tree, driver downloads, CUDA/NCCL archives and distro packages.
Cached downloads are checksum-checked again. Two builds must have identical
manifests (excluding the report timestamp) and verity hashes before publication.
This is same-input reproducibility, not proof of hardware compatibility or
cross-host reproduction. The resolved kernel snapshot is uploaded for review.

CI publishes to `ghcr.io/confidential-dot-ai/confidential-os-builder`:

| Tag | Purpose |
|---|---|
| `gpu-workload-<commit>` | confos artifact with disk, firmware and manifest |
| `gpu-workload-<commit>-cdi` | KubeVirt CDI import representation |
| `gpu-workload-latest`, `gpu-workload-cdi-latest` | Main-branch aliases only |

Feature branches named `gpu-workload-*` publish commit tags, never latest
aliases. Deploy by OCI digest, not a moving tag. Download the matching manifest
with `confos pull <artifact-reference> output/gpu-workload`; compare attestation
against `output/gpu-workload/manifest.json`. CDI and confos representations have
different OCI digests. Do not reuse the old PoC image's reference values.

## Launch contract

GPU counts are detected at runtime; qualify supported hardware layouts
individually. The image can also boot without GPUs for SSH/container diagnostics;
GPU checks and NCCL fail explicitly then.

For KubeVirt, attach:

- The read-only OS image from the matching `-cdi` artifact.
- The selected GPU PCI devices, an Intel TDX domain and VSOCK for host QGS.
- A public-key Secret disk with `volumeLabel: opkeydata` and a file named
  `pubkey`. Put one OpenSSH public key per line; supply customer and operator
  keys here, not in the image. The initrd binds the exact bundle into RTMR[3].
- A separate ext4 PVC formatted once with filesystem label `user-data`, mounted
  at `/var/lib/user-data`. Retain it when replacing the VM; never reformat it
  during recovery. Docker/containerd require that mount before starting.

**The ext4 user-data volume is not encrypted by this profile.** The host can
read or change it. Persistent container layers, models and configuration are
outside the launch measurement and need separate confidentiality/integrity
controls when required. Recovery does not attest disk contents.

SSH listens on guest port 22; the operator supplies a Service or public port
forward. There is no host-root handover or serial-root autologin. Host keys
regenerate with the ephemeral `/etc` overlay; verify the new fingerprint after
replacement. Docker's bridge MTU defaults to 1420 for the tested KubeVirt
network; lower it if the actual underlay requires that.

## Acceptance checks

After launch, from a root SSH session:

```bash
gpu-workload-check
gpu-container-check
findmnt /var/lib/user-data
df -h /var/lib/user-data
run-nccl-smoke /var/lib/user-data/results/$(date -u +%Y%m%dT%H%M%SZ)
```

`gpu-workload-check` compares B200/B300 PCI devices to initialized GPUs,
requires CC ON and Ready, rejects devtools and requires NVLE for multi-GPU
guests. `CONFOS_EXPECTED_GPUS=8 gpu-workload-check` adds an explicit allocation
check. Diagnostics are not cryptographic attestation. The NCCL matrix uses every
initialized GPU in native and MPI paths; success requires zero validation errors.

Also verify nonce-bound CPU/GPU attestation externally against this image's
manifest: launch measurements, TCB status, GPU policy and operator-key binding.
Test a digest-pinned GPU container through CDI, Docker bridge DNS/HTTPS, a
package install and small Git/PyPI downloads. Reprovision with the retained
data PVC and repeat. CI tests configuration and mocked GPU counts; it does not
replace 2-GPU and 8-GPU hardware acceptance runs.
