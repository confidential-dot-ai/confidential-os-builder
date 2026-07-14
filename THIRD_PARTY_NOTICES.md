# Third-party notices

## output/OVMF.fd

EDK2 (OVMF) firmware binary, built from the
[confidential-dot-ai/edk2](https://github.com/confidential-dot-ai/edk2) fork
(branch `OvmfPkg-PlatformPei-skip-pvalidate-igvm-pages`), which adds the
IGVM HOB region required by confos's IGVM construction. EDK2 is licensed under
BSD-2-Clause-Patent; see https://github.com/tianocore/edk2/blob/master/License.txt.
Committed so builds and CI work without a separate firmware download.
