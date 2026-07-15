# c8s node image (`c8s` profile)

Status: **in progress** on `feat/c8s-profile` (branched off
`feat/gpu-confos-profile`). This document is the design of record for the
measured CVM **node** image that runs c8s.

## Goal

One published, measured, attestable TDX node image for the node-as-CVM
deployment: one large CVM captures the
full node's resources, GPUs passed through, and `c8s install --distro rke2`
deploys onto the single-node cluster it boots. Everything that today
reaches nodes via privileged installer DaemonSets — kubelet + containerd
(via RKE2), the nri-image-policy NRI plugin, attestation-api — is baked
into the dm-verity root instead, so the launch measurement covers it.

Composition:

```
bin/build-c8s      # = confos kernel → steep-fetch-gpu → steep-fetch-attest-gpu →
                   #   confos build c8s --profile gpu --profile attest-gpu --profile c8s
                   #     --kernel-config-fragment kernel/c8s.config
                   #     --kernel-builder-package dwarves,python3,pkg-config,zlib1g-dev
                   #     --cloud-init mkosi/base/mkosi.profiles/c8s/user-data
                   #     --platform tdx --memory 16G
```

Knobs: `C8S_NO_GPU=1` (attest + c8s only, GPU-less validation),
`C8S_STOCK_ATTEST=1` (compose the stock `attest` profile instead of
`attest-gpu`: same attestation-api on :8400 but no GPU evidence collection —
CI's mode until the attestation-rs `-gpu` image digest is pinned),
`C8S_NAME`, `C8S_MEMORY`. Extra args pass through (`bin/build-c8s
--profile ssh` for bring-up debugging).

## What the profile bakes (all measured)

| Piece | Source | Where |
|---|---|---|
| RKE2 server `v1.34.5+rke2r1` | pinned+sha256 release tarball (mkosi.sync) | `/usr/local/` |
| Airgap image bundles (core + cilium) | pinned+sha256 (mkosi.sync) | `/var/lib/rancher/rke2/agent/images/` |
| nri-image-policy plugin | `ghcr.io/confidential-dot-ai/nri-image-policy@sha256:…` by digest (mkosi.sync) | `/opt/nri/plugins/10-nri-image-policy` |
| NRI enablement + fail-closed validator | baked containerd drop-in + template | `/var/lib/rancher/rke2/agent/etc/containerd/` |
| Plugin boot config (measured floor) | profile mkosi.extra | `/etc/nri/conf.d/image-policy.yaml` |
| RKE2 config: Cilium CNI, kube-proxy off, CIS, PSA, audit | ported from base-images rke2 (validated e2e under c8s) | `/etc/rancher/rke2/` + `server/manifests/` |
| containerd-data-disk service | ported from base-images rke2 | encrypted `LABEL=containerd` backing for the image cache |
| attestation-api | **attest-gpu profile** (or `attest` under `C8S_STOCK_ATTEST=1`), not this one | host service on `0.0.0.0:8400` |
| NVIDIA driver stack | **gpu profile** | see docs/GPU-IMAGE-PLAN.md |

Airgap bundles are baked deliberately: the measurement covers the exact
bytes of every control-plane image (kube-* static pods, Cilium, CoreDNS)
and first boot needs no registry egress. Cost: ~1.6G of disk.raw
(mkosi.repart's root ceiling was raised to 16G for this; `Minimize=best`
keeps other images small).

## Kernel

`kernel/c8s.config` = every effective line of `kernel/gpu.config` verbatim
(one `--kernel-config-fragment` per build; `bin/lint` enforces the
inclusion) + the container/k8s symbol set ported from base-images
`rke2/kernel/container.config` (validated e2e under c8s on the SNP rke2
image) + `# CONFIG_DEBUG_INFO_BTF_MODULES is not set` (keeps pahole out of
the NVIDIA out-of-tree module build). Everything is `=y` — nothing may
modprobe at runtime because `nvidia-modules-latch.service` sets
`kernel.modules_disabled=1` after the driver loads (the rke2-server
`no-modprobe.conf` drop-in silences rke2's modprobe attempts).

`CONFIG_DEBUG_INFO_BTF` requires pahole in the kernel-builder tools tree —
hence `--kernel-builder-package dwarves,python3,pkg-config,zlib1g-dev` —
and is Kconfig-incompatible with struct-layout randomization
(`depends on !GCC_PLUGIN_RANDSTRUCT`), so the fragment overrides
`hardening.config`'s `RANDSTRUCT_FULL` to `RANDSTRUCT_NONE`. The c8s kernel
trades struct-layout randomization for BTF, the same trade every
BTF-shipping distro kernel makes; the base/gpu images keep RANDSTRUCT.

## Launch requirements (hard, unlike the gpu image)

- **A virtio-blk disk with device serial `confai-scratch`, ≥64G.** The
  initrd backs the overlay upper with it (per-boot-key encrypted,
  ephemeral). Without it the upper is a 2G tmpfs, which cannot hold RKE2's
  `/var/lib/rancher` — rke2 wedges once the overlay fills. Local test:
  `confos run output/c8s --scratch 64G`.
- **Recommended: a second disk labeled `containerd`** (host runs
  `mkfs.ext4 -L containerd` once as the intent marker; the guest
  re-encrypts over it per boot). Keeps the multi-GiB image cache off guest
  RAM. Prefer virtio-scsi — the script scans `sd*` first; secondary
  virtio-blk disks have wedged under SEV-SNP/KubeVirt before.
- Guest memory ≥16G (etcd OOMs at small sizes). TDX measurement is
  topology-invariant, so memory/SMP can vary per launch without changing
  the reference values.
- GPU launches: DSDT MMIO window, vsock device for the QGS quote path,
  FLR-before-probe are all handled by the gpu profile + launch config —
  see docs/GPU-CC-OPERATIONS.md.

## Cluster state posture: fully ephemeral (v1)

Every boot forms a fresh single-node cluster (no join token — the
single-node-server simplification). Reboot = reprovision; `c8s install`
runs again. Persisting etcd across boots on an untrusted-host disk needs
an integrity story (the scratch disk is plain-mode dm-crypt:
confidentiality only) — out of scope for v1. Per-host hostname comes from
NoCloud `meta-data` (`local-hostname`), outside the measurement; rke2
config overrides go in `/etc/rancher/rke2/config.yaml.d/` at runtime.

## nri-image-policy trust posture

The **measured invariants** are the plugin binary, the fail-closed NRI
registration (`default_validator.required_plugins`), and the boot-time
floor config. The **policy data** (CDS measurements, the image-digest
floor) is deploy-time: after `c8s install`, the chart's installer
DaemonSet overwrites `/etc/nri/conf.d/image-policy.yaml` on the unmeasured
overlay with the values-rendered version. A CDS bump therefore needs no
image rebuild; the chain of trust for the policy data is CDS RA-TLS +
attestation-api, not the image measurement.

Baked-floor consequences before install:

- Pull loop retries against `https://127.0.0.1:30808` (no CDS yet) —
  plugin stays Ready on the floor; `cds_measurements: []` means
  accept-any-attested-CDS (logged) until the installer pins real values.
- Only `kube-system` and `local-path-storage` are exempt; the floor
  self-allows only the plugin's own image. Anything else is denied —
  that's the fail-closed proof, not a bug.

**Keep the mkosi.sync `NRI_IMAGE` digest in lockstep with the digest the
c8s chart deploys** (`nriImagePolicy.image.digest`). Identical bytes make
the installer's `cmp` a no-op; a mismatch means the installer rewrites the
binary (runtime, unmeasured) and restarts rke2-server — legal but it
downgrades the measurement story for the plugin binary.

## Installing c8s on the booted node

Run `c8s install --distro rke2` **unchanged** (kubeconfig:
`/etc/rancher/rke2/rke2.yaml` in the guest; expose 6443). What happens
against this image, all verified against the chart source:

- `containerd-prep` finds the baked `config-v3.toml.tmpl` import already
  present (exactly one `imports` line) → no-op. The baked template carries
  the prep's sentinel comment so future prep runs manage it correctly.
- The installer DS `cmp`s the plugin binary → identical (same digest) → no
  rewrite. It overwrites the boot config (expected diff: cds_measurements,
  floor) → **exactly one rke2-server restart**.
- Transient denials during install are expected: c8s components other than
  the (self-allowed) installer may be denied until the config refresh
  lands; kubelet retries converge.
- Do NOT set `nriImagePolicy.enabled=false`: the chart then stops
  rendering CDS's allowlist seed (`serveAllowlistSeed`) unless kata is on.
- The chart's attestation-api DaemonSet coexists with the baked host
  service (pod netns vs host :8400). In-cluster consumers can use either;
  the baked NRI plugin uses the baked one at `http://127.0.0.1:8400`.
- Feed this image's `manifest.json` TDX values (mrtd/rtmr1/rtmr2) into the
  deployment's expected measurements (`cds.measurements` values) so the
  mesh verifies the node it runs on.

## Validation stages

- **S0 kernel**: `bin/confos kernel --kernel-config-fragment
  kernel/c8s.config --kernel-builder-package dwarves,python3,pkg-config,zlib1g-dev`
  passes fragment verification; snapshot diff reviewed; `/sys/kernel/btf/vmlinux`
  present in the built kernel, `DEBUG_INFO_BTF_MODULES` absent.
- **S1 GPU-less validation (attest + c8s)**: `C8S_NO_GPU=1 bin/build-c8s
  --profile ssh`, `confos run --scratch 64G`. Exit: rke2-server active;
  attestation-api unit active; node Ready;
  cilium/coredns/local-path Running **from the airgap bundles**; rendered
  containerd config has exactly one `imports` line; NRI plugin registered
  (health socket answers); a pod with an unlisted image in a non-exempt
  namespace is denied; kube-system unaffected.
- **S2 full composition, GPU-less boot**: `bin/build-c8s`; nothing
  degraded (FLR no-op, persistenced skipped, latch engaged); rke2 Ready;
  attestation-api unit active.
- **S3 TDX CVM with GPUs**: launch per docs/GPU-CC-OPERATIONS.md
  + the disks above. Exit: all GPUs CC-On/Ready; `modules_disabled=1`;
  rke2 Ready; containerd cache on the encrypted disk (not tmpfs);
  `POST 127.0.0.1:8400/attest {"nvidia_gpu":true,"report_data":<nonce>}`
  returns nonce-bound TDX+GPU evidence matching `manifest.json`.
- **S4 c8s on top**: `c8s install --distro rke2` completes; one
  rke2-server restart; CDS-fed allowlist enforced (allowed digest admits,
  unlisted denies); ratls-mesh/tee-proxy functional; chart attestation-api
  coexists with baked :8400.
- **S5 CI + publish**: c8s.yml green; a no-change rerun hits the crane
  roothash skip; pulled artifact re-verifies.

## Deferred: GPU pods (follow-up)

The gpu profile makes GPUs host-visible (`nvidia-smi`, CC Ready) but NOT
schedulable as pod resources. That needs (a) image-side:
nvidia-container-toolkit (`nvidia-ctk`, `nvidia-container-runtime{,-hook}`,
`libnvidia-container`) plus either a boot oneshot generating a CDI spec
(`nvidia-ctk cdi generate` — lands on the overlay but derives from the
measured driver) or a containerd runtime drop-in; and (b) cluster-side:
nvidia-device-plugin (could be baked as a `server/manifests/` file,
digest-pinned). Do this only after S4 passes so GPU-pod breakage never
conflates with enforcement bring-up.

## Risks / open questions

1. **Preset timing for sync-staged units**: 50-rke2.preset assumes
   `systemctl preset-all` sees the rke2 units staged via mkosi.local into
   `/usr/local/lib/systemd/system`. If not, fall back to explicit
   `multi-user.target.wants` symlinks in mkosi.extra. First thing S1
   checks.
2. **rke2 version bumps vs the baked containerd template**: rke2 v1.34.5
   predates the k3s change that emits the `config-v3.toml.d` import
   itself; if a bump adds it, the rendered config gets TWO imports lines
   (containerd rejects duplicates — though prep's duplicate-handling
   removes our sentinel template at next install). Bump checklist: verify
   exactly one `imports` line on first boot.
3. **attest-gpu sentinel**: composed builds need
   `bin/steep-fetch-attest-gpu` (local attestation-rs
   `--features nvidia-gpu-attest` build) until the `-gpu` image publishes;
   CI runs `C8S_STOCK_ATTEST=1` until then, so the published image serves
   TDX quotes but not GPU evidence.
4. **CI runner disk/time**: kernel + NVIDIA + airgap + ~6G disk.raw on
   ubuntu-latest is tight; the workflow frees space, but a larger runner or
   `maximize-build-space` may become necessary.
5. **CIS + NRI first-boot ordering**: containerd starts with
   `required_plugins` before the plugin registers (10s registration
   timeout gates container creation — intended fail-closed; watch for a
   slow first boot).
6. **Snapshot lockfile churn**: `kernel/config-x86_64.snapshot` is shared
   across the base/gpu/c8s lineages; the last-built lineage rewrites it.
   `git checkout` after cross-lineage local builds; regenerate deliberately
   in the PR that changes `kernel/c8s.config`. Future fix: key the
   snapshot path on the fragment name (small Rust change, also wanted by
   gpu).
