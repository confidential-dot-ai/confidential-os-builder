# Third-party notices

## output/OVMF.fd

EDK2 (OVMF) firmware binary, `OvmfPkg/OvmfPkgX64.dsc` RELEASE GCC with
`-D SMM_REQUIRE=FALSE`, built from the
[confidential-dot-ai/edk2](https://github.com/confidential-dot-ai/edk2) fork,
branch `confos/igvm-measured-kernel`, commit `30e829293c46010f5ddef1d87fae710d07468b80`:
upstream edk2 `75b106917296fe7b82d2d14700c90197b905e2a6` (IGVM HOB region and
PVALIDATE skip) plus a fix so that, when the IGVM carries the kernel, OVMF
boots only that kernel with its measured command line.
Same bytes as `ghcr.io/confidential-dot-ai/ovmf-snp-artifacts@sha256:f3691596d7e7e889995fb6bd600e42dde4b6bb55882358d356b6176214d6c4bf`;
sha256 `de50ffdc7e973f07f74df1be206cf96547f66b211026f2a9194c5856738d4dd8`. EDK2 is licensed under
BSD-2-Clause-Patent; see https://github.com/tianocore/edk2/blob/master/License.txt.
Committed so builds and CI work without a separate firmware download.
