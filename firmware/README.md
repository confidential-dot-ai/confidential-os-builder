# Vendored TDX firmware

`OVMF.inteltdx.fd` — the unified, `-bios`-bootable Intel TDX firmware (TDVF)
confos measures for the TDX `mrtd`, and the firmware KubeVirt's virt-launcher
boots a TD with.

- **Source:** `quay.io/kubevirt/virt-launcher:v1.9.0-beta.0`,
  `/usr/share/edk2/ovmf/OVMF.inteltdx.fd` (edk2-ovmf, RHEL/CentOS build).
- **sha256:** `a7edaeff6fc4bef8924461cb0fb68a194c3595d08ccad2720cca926dea12f7cf`
- **License:** BSD-2-Clause-Patent (edk2 / TianoCore) — see `LICENSE`.

Why vendored (confidential-metal#82): Ubuntu's `ovmf` package ships only the
non-TDX `OVMF.fd` (does not `-bios`-boot a TD); `ovmf-inteltdx` is absent on
noble and ships an MS-keys `.ms.fd` on resolute (doesn't finish-boot an
unsigned UKI). Vendoring the exact firmware the guest actually boots removes
all distro/package drift and guarantees `manifest.json`'s `tdx.mrtd` equals a
live quote's MRTD. Verified: tdx-measure(this file) == 9309eaae… == the digest
a live b200 c8s CVM attests, and it boots the c8s rootdisk to Linux 6.16.12.
