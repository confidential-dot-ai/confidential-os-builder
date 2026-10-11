# Cloud images

Cloud recipes in `ci/images.json` are opt-in. They stack on `general-purpose`
(SSH, containers, attest). Default images are unchanged.

| Recipe | Platform | Tested on |
|---|---|---|
| `azure` | Azure confidential VMs, Intel TDX (DCesv6) | DC2es_v6, 3 boots incl. deallocate/start |

```bash
bin/build-image azure          # output/azure
```

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

## Root and provisioning

- The initrd finds the verity partitions by PARTUUID, so the SCSI (Azure)
  boot disk works. A scratch disk is found only on virtio-blk, so cloud
  images use the 2G RAM state tmpfs.
- cloud-init uses the cloud datasource instead of the measured NoCloud seed.
  SSH keys come from cloud metadata and are installed for root
  (`adminUsername` is not created). They are **not** launch-measured: whoever
  controls the cloud account can add a key.
- Serial console: 8250 built in, `console=ttyS0,115200n8`, no serial login.
  Enable boot diagnostics at VM create to read it.
- Boot with Secure Boot off: systemd-boot and the UKI are unsigned.

## Attestation

On Azure the TD (or SNP guest) runs under Microsoft's paravisor. Evidence is
the paravisor's HCL report plus a vTPM quote, so the `azure` profile:

- sets the baked attestation-api `platforms` to `["az-snp", "az-tdx"]`;
- lets attestation-api open `/dev/tpm0` and `/dev/tpmrm0`.

The TD's MRTD and RTMRs belong to the paravisor (RTMR[0-3] are zero). The
image's code identity is in vTPM PCR4/9/11. MRTD differs between Azure hosts:
two values were seen on one VM across a deallocate, so a verifier needs an
allow-list of paravisor MRTDs, not one pin.
