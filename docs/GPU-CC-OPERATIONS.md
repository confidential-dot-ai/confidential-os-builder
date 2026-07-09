# GPU confidential-computing images — operational notes

Hard-won behaviours of the `gpu` / `attest-gpu` profiles running NVIDIA B200s
under Intel TDX. Read this before debugging a GPU CVM that "won't come up" or
attestation that "won't quote". Each item cites where it's enforced in code.

Companion: `docs/GPU-IMAGE-PLAN.md` (design), the trusted DSDT
(`mkosi/base/acpi-tables/dsdt.asl`), and (on the operator host)
`tdx-checkpoints/030`, `031`.

## 1. CC needs a fresh FLR before the FIRST driver attach — and only one attach per FLR

In confidential-compute mode the GPU refuses to establish an SPDM session
unless the PCI function was **freshly FLR-reset**, and it allows exactly **one**
attach per reset. Two consequences the image must honour:

- **FLR before the driver's first probe.** We do NOT auto-load `nvidia` via
  `modules-load.d` (deleted) — that would let the kernel probe every GPU before
  any reset. `nvidia-gpu-flr.service` FLRs every GPU via
  `/sys/bus/pci/.../reset` (no driver bound → clean), *then* modprobes the
  driver, so its first probe lands on reset functions.
- **`nvidia-persistenced` is load-bearing, not an optimisation.** It performs
  and *holds* that one good attach for the life of the VM. If it exits, a
  transient client (`nvidia-smi`) tears the session down and the next attach
  fails — with no way to recover short of another FLR. It also needs
  `libnvidia-cfg.so` staged, or it dies at init (silently, pre-fix).
- **persistenced must start only after ALL GPUs are bound.** The per-GPU SPDM
  attach is slow and staggered; at 8 GPUs, persistenced starting the instant
  `modprobe` returns holds only the early GPUs, and the rest get torn down.
  `nvidia-gpu-flr` therefore blocks until `/proc/driver/nvidia/gpus/` lists all
  reset devices (a teardown-free check — do NOT poll with `nvidia-smi`, which
  opens each device) before it returns, so persistenced (ordered after it)
  enumerates and holds every one.
- **Gotcha — the boot activation window.** Running `nvidia-smi` *before*
  persistenced has finished holding all GPUs can tear down the not-yet-held
  sessions (seen at 8: `nvidia-smi -L` drops 8→2). Once persistenced is
  `active` and holding them, `nvidia-smi` is safe, and the attestation NVML
  collection never tears sessions down. Just don't poke GPUs during early boot.

Failure signature when this is wrong: `NVRM: osInitNvMapping: *** Cannot attach
gpu` → `RmInitAdapter failed! (0x22:0x56:894)`, `nvidia-smi: No devices were
found`. It is **silent at 1 GPU and racy at 8** (the old modules-load ordering
recovered 5/8 and left 3 stuck) — which is why it slipped past single-GPU
validation. Enforced in `usr/local/bin/nvidia-gpu-flr` +
`nvidia-persistenced.service` + `bin/steep-fetch-gpu` (stages libnvidia-cfg).

The host-side FLR that vfio does at VM-open does NOT satisfy this by the time
the guest driver attaches — the reset must happen *inside* the guest.

## 2. SPDM needs the kernel crypto API (LKCA)

The driver's SPDM stack links the Linux Kernel Crypto API (ECC/ECDH/ECDSA/KPP).
steep's minimal kernel lacked them, so the driver built libspdm against *stubs*
and CC init failed: `libspdm expects LKCA but found stubs!`. Fixed by
`CONFIG_CRYPTO_ECC/ECDH/ECDSA/KPP=y` in `kernel/gpu.config`. Only visible with a
real GPU (the GPU-less boot never reaches SPDM).

## 3. Multi-GPU BAR mapping — two knobs, only one is in the image

B200 resizable BAR2 is **256 GiB**; the 6.16 kernel keeps it at full size (stock
6.8 guests ran it at 8 GiB, which is why they "just worked" and never exercised
this). Mapping N of them needs:

- **Guest side (in the image): a high 64-bit MMIO `_CRS` window, 2..64 TiB**, in
  the trusted DSDT (`dsdt.asl`). OVMF places these BARs at ~56 TiB (default
  aperture) or ~2 TiB (raised aperture); the wide window covers both. Linux
  drops any host-bridge window that overlaps RAM *whole*, so the original low
  32 GiB..1 TiB window vanished on ≥32 GiB-RAM guests. Without this:
  `BAR0 is 0M @ 0x0 / can't claim; no compatible bridge window`.
- **Firmware side (launch config, NOT the image): the OVMF 64-bit aperture.**
  - **Raw QEMU:** the default aperture fits ~2-3 GPUs; for 4+ add
    `-fw_cfg name=opt/ovmf/X-PciMmio64Mb,string=2097152` (2 TiB). Too small →
    OVMF can't map the BARs *and the boot disk* → falls through to PXE. (8 TiB
    is too large — breaks iommufd bind; 46-bit hosts cap at 64 TiB anyway.)
  - **KubeVirt:** virt-launcher's aperture is large enough for **4× 256 GiB with
    no tuning**; the DSDT window alone suffices there. (8-GPU aperture: see the
    validation matrix.)

Also: guest RAM must avoid the low PCI window — `-m 32G` collides at exactly
`0x800000000`; use ≤16 G or ≥64 G (the working multi-GPU launches use 64-96 G).

## 4. Quote path — vsock (fast), not ConfigFS-TSM (slow)

steep's kernel dropped vsock for hardening; ConfigFS-TSM
(`/sys/kernel/config/tsm/report`) works but is ~1 s+/quote. Re-enabled
`CONFIG_VSOCKETS + CONFIG_VIRTIO_VSOCKETS` (`kernel/gpu.config`) and set
`tdx_quote_method = "vsock"`: the attestation service reaches the host QGS at
**CID 2 : 4050** in ~7 ms (CPU quote) / ~130-160 ms (with GPU evidence).

Wiring, per stack:
- **Raw QEMU:** `-device vhost-vsock-pci,guest-cid=N` + `tdx-guest`
  `quote-generation-socket vsock cid 2 port 4050`. The host QGS already listens
  on vsock 4050.
- **KubeVirt:** confai sets `Domain.Devices.AutoattachVSOCK` on TDX VMIs
  (`pkg/confai/build.go`), and the KubeVirt CR needs the **`VSOCK` feature
  gate** (`roles/kubevirt`, TDX clusters). Without both, `/attest` fails
  `vsock connect to CID 2:4050 failed: No such device`.

**Hardening:** vsock is a host↔guest channel, so it's fenced at the unit level.
`attestation-api.service` is the ONLY unit whose `RestrictAddressFamilies=`
includes `AF_VSOCK`; every other unit (nvidia-*, sshd) is fenced to `AF_UNIX`.
The image ships no vsock listeners; the sole traffic is the outbound GetQuote.

## 5. Attestation shape

`POST /attest {platform:"tdx", nvidia_gpu:true, report_data:"<≥16-byte b64>"}`
→ TDX v4 quote (report_data == the caller nonce, verbatim) + one evidence entry
per GPU. `POST /verify` (+ `params.nvidia_gpu_user_nonce`) →
`signature_valid:true` via NVIDIA NRAS. The verifier's clock must be roughly
sane (NRAS JWTs have a tight `nbf`; the minimal guest has no NTP — collection is
clock-independent, verification is not).

## Validation matrix (real B200, this session)

| GPUs | Raw QEMU / iommufd | KubeVirt / confai |
|------|--------------------|-------------------|
| 1 | ✅ CC Ready, attest | ✅ CC Ready, attest |
| 2 | ✅ NVLE, attest ×2 | ✅ NVLE, attest ×2, vsock |
| 4 | ✅ NVLE, attest ×4 (needs fw_cfg aperture) | ✅ NVLE, attest ×4, vsock (no aperture tuning) |
| 8 | (not run) | ✅ NVLE, attest ×8, vsock (no aperture tuning) |

**8-GPU** was the acid test for item 1. The first attempt attached 5/8 (the
FLR-ordering race) and then dropped held sessions (persistenced starting before
all 8 bound). Both are fixed: FLR-before-modprobe + wait-for-all-bound. Result:
all 8 attach cleanly (0 `RmInitAdapter` failures), persistenced holds all 8, and
`POST /attest {nvidia_gpu:true}` returns 8 evidence entries in ~0.33s with no
teardown. Aperture is NOT the limiter at 8 — KubeVirt's virt-launcher maps 8×
256 GiB fine (the failures were CC/FLR, not `BAR0 is 0`).

## KubeVirt parity checklist (for `confai launch` on TDX+GPU)

- [x] Multi-GPU BAR window — in the image (DSDT), no per-launch tuning ≤4 GPU.
- [x] vsock — `autoattachVSOCK` (confai) + `VSOCK` gate (KubeVirt CR).
- [x] 8-GPU FLR ordering + session retention — fixed (FLR-before-modprobe + wait-for-all-bound); all 8 attest.
- [ ] Publish `attestation-api-gpu` (Dockerfile.gpu) + pin its digest in
      `attest-gpu/mkosi.sync` (currently pre-staged via `steep-fetch-attest-gpu`).
- [ ] Guest time source (NTP or host-time) so in-guest `/verify` isn't skewed.
