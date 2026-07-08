# GPU base image plan (`gpu` profile)

Status: **in progress** on `feat/gpu-profile`. This document is the design of
record for baking an attestable NVIDIA GPU confidential-VM image with steep.

## Goal

One published, measured, attestable TDX GPU base image —
`ghcr.io/confidential-dot-ai/steep/gpu-base` — that:

- confai/KubeVirt imports via CDI (the same `--cdi` single-layer path the
  CPU-only `tdx-cpu-image-cdi` already uses),
- boots with NVIDIA B200s passed through,
- reaches NVIDIA confidential-compute **Ready** state automatically at boot
  (no in-guest driver install, no DKMS, no network),
- serves nonce-bound TDX **and** per-GPU SPDM attestation evidence on `:8400`.

The whole thing rides steep's existing extension points — a kernel config
fragment plus mkosi profiles — with the single genuinely new piece being an
out-of-tree kernel-module build step (`bin/steep-fetch-gpu`).

## Why baked, not runtime-installed

The manual reference guest (`tdx-vm/tdx-guest-root.qcow2`) installs the driver
at runtime via `apt` + DKMS. That is wrong for the confidential threat model:
runtime-installed kernel modules land on the unmeasured overlay upper layer, so
they are **unattested** — a host that swaps the driver bytes is invisible to a
verifier. Baking the driver into the dm-verity root means the module bytes are
part of the roothash → UKI → RTMR[2] chain, so attestation covers them.

Secondary win: building the modules against our kernel at *image-build* time
means the guest never needs kernel headers, gcc, or DKMS — hundreds of MB of
toolchain and attack surface stay out of the image.

Cost trade: ~3–5 min added to each image build (module compile) vs. ~5–10 min
+ network + toolchain on *every* VM launch. Baked wins on both attestation and
aggregate time.

## Design decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | Ship **`kernel/gpu.config`** as a steep in-repo fragment, wired through the `build-gpu` Make target via `--kernel-config-fragment`. | GPU is a first-class steep product; one kernel lineage, one auditable fragment diff. |
| D2 | **Bump the guest kernel 6.12.94 → 6.16.12** (already validated on `feat/kernel-6.16-tdx-rtmr`, folded into this branch). | 6.16 exposes the TDX RTMR-extend sysfs (`/sys/.../tdx_guest/measurements/rtmrN`) for runtime per-workload measurement, and the working B200 CC guest runs 6.17-generic with driver 595.71.05 — so 595 builds cleanly at ≤6.17. 6.16.12 is comfortably inside that window; going to 6.18 risks NVIDIA-595 driver-build breakage for no benefit a regularly-rebuilt base image needs. |
| D3 | `gpu.config` = **`CONFIG_MODULES=y` + `MODULE_SIG*`** and nothing more (the symbols NVIDIA needs — `MMU_NOTIFIER`, `X86_PAT`, `MTRR`, `FW_LOADER`, `DMA_SHARED_BUFFER` — already resolve `=y`). steep's `mod2yesconfig` keeps every in-tree driver built-in, so the **only** loadable modules in the system are the two signed NVIDIA ones. | Minimal, auditable hardening delta; monolithic posture preserved. |
| D4 | **Integrity = dm-verity, not a signing PKI.** Modules are signed only because lockdown-confidentiality refuses to load unsigned modules — and they're signed with the kernel build's **auto-generated ephemeral key** (`certs/signing_key.pem`), discarded with the build tree. No committed key, no CI secret, no key management. | The module bytes are already measured by dm-verity; the signature is a lockdown formality, not the trust root. Trade-off: signature bytes differ per build, so gpu-image rebuilds aren't bit-identical — attestation is unaffected (the manifest records this build's measurements). Reverting to bit-reproducible is a one-line pinned-key change if ever wanted. |
| D5 | After the driver loads at boot, latch **`kernel.modules_disabled=1`**. | Restores the no-further-modules posture once NVIDIA is in. |
| D6 | Kernel modules: **open-gpu-kernel-modules `595.71.05`** (tag verified, commit `095de56d`), built out-of-tree, **only `nvidia.ko` + `nvidia-uvm.ko`**. | Open modules are mandatory for Blackwell CC; drm/modeset/peermem excluded — headless compute needs neither and it keeps `CONFIG_DRM` off. |
| D7 | Userspace from the **`.run` installer** (`NVIDIA-Linux-x86_64-595.71.05.run`, 403 MB, verified live, pinned by sha256), extracted and cherry-picked: `libnvidia-ml`, `nvidia-smi`, `nvidia-persistenced`, `nvidia-modprobe`, and the GSP firmware blobs → `/lib/firmware/nvidia/595.71.05/`. | Exact version-lock to the modules; sidesteps the distro mismatch (base is Ubuntu resolute/26.04; NVIDIA's apt repo targets ubuntu2404). |
| D8 | **No CUDA, no NCCL, no in-guest Fabric Manager** in the base image. | CUDA/NCCL are workload-layer (vLLM wheels bundle their runtime). Per checkpoints 006/012 the load-bearing Fabric Manager is the **host** one; the in-guest copy is added to the profile only if Stage 5's 8-GPU NVLE test proves it necessary. |
| D9 | GPU attestation: a new **`-gpu` build of `attestation-api`** (`--features nvidia-gpu-attest` + `libnvat.so`), pulled by digest into an **`attest-gpu`** profile. | The GPU collector is already merged on `attestation-rs` `main`; the published image just isn't built with the feature. `libnvat` compiles from NVIDIA's public attestation-sdk (pinned rev `0c1be386`, matching the Cargo dep) with no GPU needed at build time, so CI can produce it. The caller's `report_data` nonce transitively binds the CPU quote **and** every GPU (`gpu_nonce = SHA256(nonce ‖ "NVIDIA-GPU-EAT-v1")`) — closing the `report_data=0` replay gap in today's TDX-only image. |

## Repo layout (this branch)

```
kernel/gpu.config                          MODULES + MODULE_SIG fragment
kernel/version                             6.16.12 (folded in from feat/kernel-6.16-tdx-rtmr)
bin/steep-fetch-gpu                        fetch(pinned sha)→build in kernel-builder nspawn→sign→depmod→stage
mkosi/base/mkosi.profiles/gpu/
  mkosi.conf                               apt: kmod, pciutils; doc header
  mkosi.extra/etc/modules-load.d/nvidia.conf
  mkosi.extra/etc/systemd/system/nvidia-persistenced.service        (--uvm-persistence-mode baked)
  mkosi.extra/etc/systemd/system/nvidia-cc-ready.service            (oneshot conf-compute -srs 1)
  mkosi.extra/etc/systemd/system/nvidia-modules-latch.service       (sysctl modules_disabled=1)
mkosi/base/mkosi.profiles/attest-gpu/
  mkosi.conf + mkosi.sync                  pulls the -gpu attestation-api + libnvat.so by digest
Makefile                                   build-gpu target chains the three steps
.github/workflows/gpu.yml                  base.yml clone + fetch-gpu step; pushes gpu-base
docs/GPU-IMAGE-PLAN.md                     this file
```

Build sequencing (the one non-obvious bit): NVIDIA modules must compile against
the *built* kernel tree, which persists at `output/kernel/build/linux-6.16.12/`
(with `Module.symvers` + `certs/signing_key.*`). So `build-gpu` runs
`steep kernel --kernel-config-fragment kernel/gpu.config` first, then
`bin/steep-fetch-gpu` (builds/signs/stages modules + userspace into
`mkosi.local/mkosi.extra/`), then `steep build … --kernel-config-fragment
kernel/gpu.config --profile gpu`. That final `steep build` hits the kernel
**cache** (same fragment fingerprint) so it does not rebuild and clobber the
tree, and `mkosi.local/` survives into the mkosi run (build.rs deliberately
does not wipe it).

## Stages & exit criteria

- **Stage 0 — baseline.** steep builds on b200-dev-1 (needs `bin/setup`:
  mkosi/oras/iasl — the only host mutation in the plan, all benign apt/uv
  packages). Exit: clean `steep build --platform tdx` of the plain base.
- **Stage 1a — kernel bump.** ✅ config folded in (6.16.12). Validation: build
  the **plain base** on 6.16.12 and boot it end-to-end *before* any GPU changes,
  so kernel-bump breakage and NVIDIA-symbol breakage never get conflated.
  Expected: all measured values (MRTD/RTMR references) change.
- **Stage 1 — gpu.config.** Fragment resolves; snapshot diff reviewed; steep's
  fragment-verification guardrail confirms no requested symbol silently dropped.
- **Stage 2 — modules.** `nvidia.ko` + `nvidia-uvm.ko` build + sign against the
  tree in the kernel-builder nspawn (same toolchain → no compiler mismatch).
  Exit: `modinfo` shows the signer, signature verifies.
- **Stage 3 — image.** Builds and boots with **nothing degraded when no GPU is
  present** (units conditioned on hardware). Then boots under KubeVirt on b200
  with 1 GPU: `nvidia-smi` lists the B200, `CC State: ON`, `Ready` after the
  oneshot, module latch engaged.
- **Stage 4 — attestation.** In-guest `POST /attest {nvidia_gpu:true,
  report_data:<nonce>}` → TDX quote nonce-bound (fixes report_data=0) + GPU EAT
  verifies against NRAS. (NRAS egress is needed only at *verify* time, not to
  serve evidence.)
- **Stage 5 — scale + publish.** 8-GPU and 4×2 partition boots; `h2d-multithread`
  sanity vs. checkpoint baselines; settle the in-guest-FM question empirically;
  `steep push --cdi` → GHCR; CI workflow; checkpoint 030; PRs on both repos.

## Risks / open questions

1. **Kernel-symbol iteration** (main risk, contained): NVIDIA OOT vs. the trimmed
   6.16 config. `MMU_NOTIFIER`/`PAT`/`MTRR`/`FW_LOADER`/`DMA_SHARED_BUFFER` are
   confirmed `=y`; UVM HMM symbols (`HMM_MIRROR`/`ZONE_DEVICE`/`DEVICE_PRIVATE`)
   are *absent* and added to `gpu.config` only if the driver build/load demands
   them (x86 passthrough UVM typically does not).
2. **Device-node creation** without a full udev stack — setuid `nvidia-modprobe`
   vs. an explicit oneshot mknod; resolved at Stage 3.
3. **Rootfs sizing** — driver userspace + GSP firmware ~1–2 GB; may need a
   `mkosi.repart` minsize bump in the gpu profile.
4. **Guest FM for NVLE** — assumed host-only (D8); Stage 5 has the falsifying test.
5. **Downstream measurement policy** — the gpu image has its own MRTD/RTMR set;
   confai's expected-measurements need the new manifest values (follow-up task).
6. **Stretch (perf):** fold the SWIOTLB max-segment 256K→2M rebuild (HANDOVER
   open question #5) into `gpu.config` — our custom kernel is exactly where that
   fix belongs.
7. **Snapshot lockfile churn:** steep hardcodes a single
   `kernel/config-x86_64.snapshot`, but we now have two kernel lineages (base
   and modules-enabled gpu). `make build-gpu` rewrites the snapshot to the gpu
   config — expected per steep's documented behavior (`git checkout` it if the
   gpu build was a one-off; CI never commits it). If this becomes annoying, a
   small Rust change to key the snapshot path on the fragment name would give
   each lineage its own lockfile. Not doing that yet.

Rough effort: 2–3 focused days, dominated by Stage 1–3 iteration on b200-dev-1.
