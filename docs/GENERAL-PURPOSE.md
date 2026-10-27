# General-purpose images

`general-purpose` and `general-purpose-gpu` are opt-in, operator-managed CVM
environments for CPU/GPU applications and containers. They contain no customer
keys, models, IPs or GPU counts. These images are distinct from
the hardened base and the serial-autologin `dev` profile.

## Contents and policy

- Both images: Docker, containerd, runc, Git, Python, package installs and SSH.
- Memory/swap, CPU-quota and PID-limit support; configure limits per container.
  `container-runtime-check` rejects runtimes missing these capabilities.
- GPU only: NVIDIA CDI, open driver `595.71.05` (from `gpu`), CUDA `13.2.2`, NCCL `2.29.7`,
  OpenMPI and checksum-pinned `nccl-tests`.
- Git, Python pip/venv, build tools and normal package-manager metadata.
- Key-only SSH, including the launch-bound `opkeydata` public-key bundle.
- CPU attestation from `attest`; GPU attestation from `attest-gpu`. The attest
  port (8400) takes a nonce only and answers with the SSH host keys and a
  quote over both; `attestation-api` itself listens on `127.0.0.1:8401`. See
  [SSH host keys of a general-purpose guest](VERIFYING.md#ssh-host-keys-of-a-general-purpose-guest).
- Docker bridge/host networking, DNS, forwarding and NAT. No guest egress
  allowlist or inbound firewall policy is installed. Host/provider networking
  and KubeVirt port exposure remain separate responsibilities. Authenticate
  and encrypt application traffic; prefer localhost-bound services plus SSH
  tunnels until an application access policy is configured.

The shared kernel policy enables privileged cgroup BPF, user namespaces, veth,
bridges, netfilter and io_uring. CRIU, kcmp, userfaultfd, FUSE and TUN stay off;
system-mapping sealing stays on. The CPU kernel keeps modules and vsock off.
GPU adds the existing signed-module/latch and vsock policy. Lockdown, trusted
AML, measured boot and lower-root dm-verity remain. Unprivileged BPF is disabled.
See the [baseline kernel security policy](KERNEL-SECURITY.md) and the
[CPU](../mkosi/base/mkosi.profiles/general-purpose/README.md) and
[GPU](../mkosi/base/mkosi.profiles/general-purpose-gpu/README.md) profile READMEs
for exact changes, reasons and limitations. Each candidate requires fresh
acceptance against its own manifest.

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

On a Linux build host with `bin/setup` prerequisites:

```bash
bin/build-image general-purpose
bin/build-image general-purpose-gpu
```

GPU builds need `MODULE_SIG_KEY` or `MODULE_SIG_KEY_PEM` matching the committed
public certificate. A local test certificate can be selected with
`--module-signing-cert /path/to/test.crt`; it changes the image measurements.
Never commit or bake the private key. CPU builds require no signing secret.
Recipes, profile composition, platform and build-time memory are registered in
[`ci/images.json`](../ci/images.json); local and CI builds use the same wrapper.

Sync/post-install hooks fetch verified CUDA/NCCL inputs and automatically
compile MPI and non-MPI tests. The build memory setting is not a GPU-count
setting. TDX CPU, memory and device layouts still need hardware acceptance.

The `Build General-Purpose Images` workflow selects recipes affected by changed
profiles. Shared configuration edits rebuild all affected recipes. It uses the
large runner and shared caches for tools, kernel sources and distro packages;
compiled kernels remain input/lineage-specific. GPU downloads, unsigned modules
and CUDA/NCCL inputs are reused where their compilation/checksum inputs match.
Unsigned modules are always copied and freshly signed before image staging.
The [cache/selection design](IMAGE-CI.md) explains cache scope and trust.
Cached downloads are checksum-checked again. Two builds must have identical
manifests (excluding the report timestamp) and verity hashes before publication.
This is same-input reproducibility, not proof of hardware compatibility or
cross-host reproduction. The resolved kernel snapshot is uploaded for review.

CI publishes to `ghcr.io/confidential-dot-ai/confidential-os-builder`:

| Tag | Purpose |
|---|---|
| `general-purpose-<commit>`, `general-purpose-gpu-<commit>` | confos artifacts with disk, firmware and manifest |
| `<image>-<commit>-cdi` | KubeVirt CDI import representation |
| `<image>-latest`, `<image>-cdi-latest` | Main-branch aliases only |

Feature branch builds do not publish by default. A manual dispatch with
`publish=true` enables commit-tagged candidate publication after CI checks and
reproducibility comparison, allowing retrieval for hardware acceptance.
Publication alone does not approve stable release or customer handoff; complete
the profile's runtime and review gates first. Only main updates latest aliases.
Deploy by OCI digest, not a moving tag. Download the matching manifest
with `confos pull <artifact-reference> output/general-purpose-gpu`; compare attestation
against `output/general-purpose-gpu/manifest.json`. CDI and confos representations have
different OCI digests. Do not reuse a different image's reference values.

## Launch contract

GPU counts are detected at runtime; qualify supported hardware layouts
individually. The image can also boot without GPUs for SSH/container diagnostics;
GPU checks and NCCL fail explicitly then.

For KubeVirt, attach:

- The read-only OS image from the matching `-cdi` artifact.
- An Intel TDX domain. GPU images also need selected PCI devices and VSOCK
  for host QGS; the CPU image uses the ConfigFS TSM quote path without vsock.
- A public-key Secret disk with `volumeLabel: opkeydata` and a file named
  `pubkey`. Put one OpenSSH public key per line; supply customer and operator
  keys here, not in the image. The initrd binds the exact bundle into RTMR[3].
  It may also carry one workload line; see [OCI workloads](#oci-workloads).
- A separate ext4 PVC formatted once with filesystem label `user-data`, mounted
  at `/var/lib/user-data`. Retain it when replacing the VM; never reformat it
  during recovery. Docker/containerd require that mount before starting.

**The ext4 user-data volume is not encrypted by this profile.** The host can
read or change it. Persistent container layers, models and configuration are
outside the launch measurement and need separate confidentiality/integrity
controls when required. Recovery does not attest disk contents.

SSH listens on guest port 22; the operator supplies a Service or public port
forward. There is no host-root handover or serial-root autologin. Host keys
regenerate with the ephemeral `/etc` overlay at every boot; fetch them with a
quote from the attest port and verify them again after a restart or a
replacement. Docker's bridge MTU defaults to 1420 for the tested KubeVirt
network; lower it if the actual underlay requires that.

## OCI workloads

A general-purpose guest can run one OCI image of the launcher's choosing, so
an instance is "this container image" without anyone logging in. Add one
line to the `pubkey` file of the `opkeydata` disk, after any SSH key lines:

```text
# confos-oci-workload/v1 {"image":"ghcr.io/org/app@sha256:<64 hex>","command":["serve"],"env":{"PORT":"8080"}}
```

`image` is required and names the image by registry, repository and digest;
a tag alone is refused. `command` (arguments after the image) and `env` are
optional. sshd reads the line as a comment, and the file may hold no SSH key
at all, in which case nobody can log in.

At boot, `general-purpose-oci-workload-prepare` moves Docker's data root to
`/var/lib/docker-scratch` (encrypted scratch, or the RAM tmpfs without a
scratch disk) before Docker starts, and `general-purpose-oci-workload` then
pulls the image by digest and runs it as the container `confos-workload`
with `--network host --restart unless-stopped`, adding
`--device nvidia.com/gpu=all` on a GPU guest. A failed pull is retried every
10 s. Each boot starts from empty Docker state and pulls again, so Docker
checks the digest on every boot and no image layer lives on the
host-readable user-data disk.

**What the quote proves.** The initrd binds the exact bytes of `pubkey`, the
workload line included, into RTMR[3] on TDX and checks them against HOSTDATA
on SNP. A verifier that recomputes the launch binding from the SSH keys and
the workload line it expects has checked which image the guest was told to
run, by digest. As with any general-purpose guest, attestation covers the
launch, not what a logged-in root does afterwards.

The line is on a host-visible disk: never put a secret in `env`. Images are
pulled anonymously, so private registries need credentials delivered some
other way (for example over attested SSH).

## Acceptance checks

After launch, from a root SSH session:

```bash
container-runtime-check
findmnt /var/lib/user-data
df -h /var/lib/user-data
# GPU image only:
general-purpose-gpu-check
gpu-container-check
run-nccl-smoke /var/lib/user-data/results/$(date -u +%Y%m%dT%H%M%SZ)
```

`general-purpose-gpu-check` compares B200/B300 PCI devices to initialized GPUs,
requires CC ON and Ready, rejects devtools and requires NVLE for multi-GPU
guests. `CONFOS_EXPECTED_GPUS=8 general-purpose-gpu-check` adds an explicit allocation
check. Diagnostics are not cryptographic attestation. The NCCL matrix uses every
initialized GPU in native and MPI paths; success requires zero validation errors.

Also verify nonce-bound CPU/GPU attestation externally against this image's
manifest: launch measurements, TCB status, GPU policy and operator-key binding.
Test a digest-pinned GPU container through CDI, Docker bridge DNS/HTTPS, a
package install and small Git/PyPI downloads. Reprovision with the retained
data PVC and repeat. CI tests configuration and mocked GPU counts; it does not
replace 2-GPU and 8-GPU hardware acceptance runs.
