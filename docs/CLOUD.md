# Cloud images

Cloud recipes in `ci/images.json` are opt-in. They stack on `general-purpose`
(SSH, containers, attest). Default images are unchanged.

| Recipe | Platform | Tested on |
|---|---|---|
| `azure` | Azure confidential VMs, Intel TDX (DCesv6) | DC2es_v6, 3 boots incl. deallocate/start |
| `gcp` | Google Compute Engine C3, Intel TDX | c3-standard-4, 3 boots incl. stop/start |

```bash
bin/build-image azure          # output/azure
bin/build-image gcp            # output/gcp
```

## Packaging and import

```bash
# GCE: gzip tar (oldgnu) holding exactly disk.raw
bin/cloud-image gcp output/gcp                       # output/gcp/cloud/confos-gcp.tar.gz
gcloud storage cp output/gcp/cloud/confos-gcp.tar.gz gs://BUCKET/
bin/cloud-import-gcp NAME gs://BUCKET/confos-gcp.tar.gz --project P

# Azure: fixed VHD, virtual size a whole MiB, uploaded as a page blob
bin/cloud-image azure output/azure                   # output/azure/cloud/confos-azure.vhd
bin/cloud-upload-azure 'https://ACCOUNT.blob.core.windows.net/vhds/confos.vhd?SAS' \
    output/azure/cloud/confos-azure.vhd
bin/cloud-import-azure RG GALLERY DEFINITION VERSION STORAGE_ACCOUNT_ID \
    https://ACCOUNT.blob.core.windows.net/vhds/confos.vhd
```

Each package is written with a `.sha256` file. The GCE image gets the guest
OS features `TDX_CAPABLE`, `UEFI_COMPATIBLE`, `GVNIC` and
`VIRTIO_SCSI_MULTIQUEUE`. The Azure image is a Gen2 Compute Gallery image
with `SecurityType=ConfidentialVMSupported`. Create VMs with Secure Boot off
(systemd-boot and the UKI are unsigned). Packaging and upload do not change
the disk bytes, so the measurements in `manifest.json` still apply.

Azure SEV-SNP (DCasv5/ECasv5) uses the same paravisor and image, and the
attestation service accepts `az-snp`, but no confos image has booted there yet.

## ACPI

The kernel fragment selects the AML the kernel may run with one
`# confos-acpi:` line (docs/KERNEL_CONFIGURATION.md).

- **Azure** uses `trusted kernel/acpi/azure-dsdt.aml` (sha256 `309fd2df...`).
  Azure does not measure ACPI (RTMR[0] is zero, no handoff event), so the gate
  is the only ACPI protection. The captured DSDT was identical across reboots,
  redeploys and DC2/DC4 sizes; Azure has no SSDT. If Azure changes its DSDT,
  the image keeps running the captured table. That is safe only while the
  devices it describes (VMBus, vTPM, UARTs) stay at the same addresses.
  Re-capture and rebuild to adopt a new table.

- **GCE** uses `host` on purpose: the firmware's DSDT/SSDTs run, and the
  verifier pins RTMR[0] per machine type. GCE measures its ACPI tables into
  RTMR[0] (CCEL event log), so the verifier compares RTMR[0], or the ACPI
  event digests in a CCEL bound to the quote, with values reviewed for that
  machine type. Without that check, the host controls the AML the kernel
  runs. Even with it, host AML runs before any verifier sees a quote; the
  check lets a verifier refuse the node, not stop the AML. GCE's DSDT, FACP
  and TPM SSDT were stable across sizes.

## Root and provisioning

- The initrd finds the verity partitions by PARTUUID, so the SCSI (Azure)
  and NVMe (GCE) boot disks work. A scratch disk is found only on virtio-blk, so cloud
  images use the 2G RAM state tmpfs.
- cloud-init uses the cloud datasource instead of the measured NoCloud seed.
  SSH keys come from cloud metadata and are installed for root
  (Azure's `adminUsername` is not created). They are **not** launch-measured:
  whoever controls the cloud account can add a key. On GCE, only `ssh-keys`
  lines for the user `cloudinit` are accepted (`cloudinit:ssh-ed25519 ...`).
  The GCE datasource reads metadata from `169.254.169.254`, not
  `metadata.google.internal`: names do not resolve yet in cloud-init's first
  stage, and with the name it waited 30 s for DHCP on `sit0` every boot.
- Serial console: 8250 built in, `console=ttyS0,115200n8`, no serial login.
  On Azure, enable boot diagnostics at VM create to read it.
- GCE only: the cmdline carries `init_on_free=0`. With init_on_free on, the
  kernel resets in the TDX MapGPA hypercall that shares the SWIOTLB buffer,
  and GCE reboots it in a loop. The cause is not known; this is a workaround.
  init_on_alloc stays on.
- Boot with Secure Boot off: systemd-boot (Azure) and the UKI are unsigned.

## Boot path

The `azure` image boots like every other confos image: the firmware starts
systemd-boot at `\EFI\BOOT\BOOTX64.EFI`, and systemd-boot starts the UKI.
The `gcp` image sets `Bootloader=uki`, so `\EFI\BOOT\BOOTX64.EFI` is the
UKI itself and the ESP has no systemd-boot.

This is for GCE's TDX firmware only. On c3-standard-4 TDX, a gcp image with
systemd-boot hangs right after the firmware prints "UEFI: Attempting to start
image". The same disk with only `BOOTX64.EFI` replaced by the UKI boots and
attests (boot and reboot; RTMR[1] matches the direct-UKI prediction). The
same systemd-boot disk boots on GCE SEV-SNP and on Azure. The firmware then
measures one image load into RTMR[1] instead of two.

## Attestation

On GCE the guest is a plain TD under Google's firmware. attestation-api
returns a TDX quote and the CCEL (`platform = tdx`). MRTD is Google's
firmware, and Google publishes a signed launch endorsement for it. RTMR[0]
holds Google's firmware configuration and ACPI and depends on the machine
type. RTMR[1] and RTMR[2] hold the image's boot chain and UKI.

On Azure the TD (or SNP guest) runs under Microsoft's paravisor. Evidence is
the paravisor's HCL report plus a vTPM quote, so the `azure` profile:

- sets the baked attestation-api `platforms` to `["az-snp", "az-tdx"]`;
- lets attestation-api open `/dev/tpm0` and `/dev/tpmrm0`.

The TD's MRTD and RTMRs belong to the paravisor (RTMR[0-3] are zero). The
image's code identity is in vTPM PCR4/9/11. MRTD differs between Azure hosts:
two values were seen on one VM across a deallocate, so a verifier needs an
allow-list of paravisor MRTDs, not one pin.
