# Third-party notices

## output/OVMF.fd

EDK2 (OVMF) firmware binary, `OvmfPkg/OvmfPkgX64.dsc` RELEASE GCC with
`-D SMM_REQUIRE=FALSE`, built from the
[confidential-dot-ai/edk2](https://github.com/confidential-dot-ai/edk2) fork,
branch `confos/base`, commit `fea497f95617747985226ee6629ac0a370a23bb5`:
upstream edk2 `75b106917296fe7b82d2d14700c90197b905e2a6` (IGVM HOB region and
PVALIDATE skip) plus a fix so that, when the IGVM carries the kernel, OVMF
boots only that kernel with its measured command line, and a fix in
`VirtioSerialDxe` that unmaps a virtio ring before freeing it (the leaked
mapping corrupted the DXE heap at ExitBootServices under `IoMmuDxe`).
sha256 `53c3bdc6654708a7b23f5db36c0548de4975322e5838e90bd258e082394bf8c0`. EDK2 is licensed under
BSD-2-Clause-Patent; see https://github.com/tianocore/edk2/blob/master/License.txt.
Committed so builds and CI work without a separate firmware download.
