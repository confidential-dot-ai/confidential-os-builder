# Plan: admit only measured AML in confidential guests

Date: 2026-09-14. Status: approved and implemented on `fix/trusted-aml`, stacked on kernel update #129 (Linux 6.18.49). Validation procedures are recorded in `docs/VERIFYING.md`; consumer rollout and hardware acceptance remain separate.

## Objective and priority

Address AUDIT-3: prevent the customer-controlled host from supplying AML that Linux interprets inside the confidential guest. This must hold before namespace loading or method execution, rather than relying on a later userspace inspection.

This is the next builder change because the current RTMR0-unpinning argument assumes that host AML has already been excluded. Bootstrap credential authority and CDS endpoint identity remain separate, high-priority c8s work. Mutable-storage integrity is also unresolved; immutable root alone is a partial mitigation.

Implement on an isolated branch. Preserve the current `ipe` branch and audit artifacts. Coordinate with [kernel update #129](https://github.com/confidential-dot-ai/confidential-os-builder/pull/129): validate against the exact maintained-kernel revision that will ship. Do not silently assume a patch researched on Linux 6.16.12 applies to 6.18.49, and do not duplicate the kernel-version change already proposed there.

## Behavior at planning time

- The current image builder compiles `mkosi/base/acpi-tables/dsdt.asl`, prepends it in an early initrd archive, and relies on `CONFIG_ACPI_TABLE_UPGRADE`.
- That upgrade matches host-controlled identifiers and requires a newer replacement revision. It is a firmware repair mechanism, not unconditional exclusion of untrusted tables. [Upstream documentation](https://docs.kernel.org/admin-guide/acpi/initrd_table_override.html).
- Linux's built-in custom DSDT mechanism offers a replacement path based on the DSDT signature without the same OEM/revision comparisons. Compile the trusted table into the measured kernel using this existing mechanism. [Upstream custom-DSDT documentation](https://kernel.org/doc/html/v5.18/admin-guide/acpi/dsdt-override.html).
- The `acpi_no_static_ssdt` option is insufficient by itself: it is limited to static SSDTs and does not prohibit dynamic installation. Exact-source review also identifies the secondary PSDT/OSDT paths. [Kernel parameter documentation](https://www.kernel.org/doc/html/latest/admin-guide/kernel-parameters.html?highlight=nohz_full).
- `src/commands/kernel.rs` already builds from a checksum-verified clean source tree, stages deterministic inputs, and fingerprints them. `src/kernel/config.rs::verify_builder_invariants` can reject consumer-fragment overrides even when ordinary last-fragment-wins validation would accept them.

The exact Linux 6.16.12 trace identifies `drivers/acpi/acpica/nsload.c::acpi_ns_load_table()` as the shared pre-parse boundary for static namespace loads and dynamic `Load`/`LoadTable`/`acpi_load_table()` paths. The candidate gate belongs before owner allocation and `acpi_ns_parse_table()`. `tbxfload.c` can copy the DSDT through `acpi_tb_copy_dsdt()`, so simple pointer equality would reject a legitimate copy: retain the resident, measured built-in array or explicitly preserve trusted provenance across copying. Keep referenced trusted bytes resident. These hooks must be rechecked on the shipping kernel. [Upstream namespace loader](https://github.com/gregkh/linux/blob/v6.16.12/drivers/acpi/acpica/nsload.c), [table loading](https://github.com/gregkh/linux/blob/v6.16.12/drivers/acpi/acpica/tbxfload.c), [installation and override](https://github.com/gregkh/linux/blob/v6.16.12/drivers/acpi/acpica/tbinstal.c).

## Approved security contract

Only the built-in, measured DSDT may enter the AML namespace. Host DSDT identifiers or revision must not select an alternative. Firmware SSDT/PSDT/OSDT and dynamic table loads must not add AML, even if their headers claim to be a DSDT or duplicate a trusted table's identifiers.

Failure to establish the trusted primary DSDT is fatal before namespace execution. Additional untrusted tables may be rejected without preventing an otherwise valid guest from booting, provided the rejection happens before any AML namespace work. Rejection behavior must be explicit and tested, not a warning followed by fallback.

A mandatory-policy failure must not merely disable ACPI and let the guest continue into RKE2. Tests must also cover legitimate DSDT copies, repeated loads and attempted unload/reload.

Retain host-provided non-AML topology data needed for CPU, memory and PCI operation. This work does not assert that parsing that data is risk-free, that all firmware attack surface disappears, or that RTMR0 has no other security relevance.

The restriction is a production build invariant. Do not provide an unmeasured runtime switch that weakens it. A deliberately different development image would need a separate measured policy and explicit verifier treatment.

## Implementation steps

1. **Pin the kernel enforcement points on the shipping kernel.** Trace the built-in override, primary and secondary namespace loaders, installation APIs, and AML `Load`/`LoadTable`. Confirm when tables are copied, how provenance survives those copies, and when init-only memory is freed. Record why every entrypoint is covered. The gate must not trust a table's signature/OEM metadata as proof of provenance.
2. **Compile the trusted DSDT during the kernel build.** Move its ASL source under `kernel/`; use the snapshot-pinned `acpica-tools` package inside the existing kernel-builder tools tree to produce the kernel's custom DSDT header. Fail on missing/invalid compiler output. Keep generation deterministic and independent of host tool versions.
3. **Add a narrowly scoped kernel enforcement patch.** Reuse the built-in DSDT override, then enforce the trusted-table contract at the namespace-loading boundary and prohibit dynamic additions. Avoid a broad ACPI fork. Patch application must fail on unsupported source or mismatching context; build from the already checksum-verified, freshly extracted tree. Test malformed/missing primary-table handling without a host-table fallback. Determine the smallest complete hook set from step 1 before editing the patch.
4. **Close alternative loaders and configuration overrides.** Disable initrd table upgrade, ACPI configfs injection and EFI SSDT overlays. Require the expected custom DSDT path and the kernel gate's enabled configuration in `verify_builder_invariants`, including scalar-path checks that the current boolean fragment checker does not perform. Reject a consumer fragment that weakens these invariants.
5. **Include all new inputs in provenance and cache identity.** Hash the trusted ASL and ordered kernel patch set in the kernel fingerprint and canonical serialization. Old manifests must cause a cache miss. Ensure local and CI kernel caches include the same inputs. A change to either the table or the enforcement patch must rebuild the kernel and change the resulting attested artifact.
6. **Remove the old early-initrd override pipeline.** Stop compiling ASL with host `iasl` during image assembly. Remove the unused early ACPI archive construction and its obsolete tests. Preserve initrd gzip timestamp normalization and any archive helpers used elsewhere. Continue measuring exactly the initrd actually embedded in the image.
7. **Update manifests and documentation.** Record the enforced AML policy and trusted-table provenance in existing kernel/image metadata where needed for consumers. Replace statements about a successful early override with the actual built-in table and loader restriction. Document measurement changes, supported topology assumptions and remaining non-AML host inputs.
8. **Validate and roll out.** Complete the tests below; publish through the normal release path only after review. Update c8s's builder pin, regenerate its kernel snapshot and images, derive reference values from those artifacts, and require hardware acceptance before retiring the old finding for a deployment.

## Expected files

| File / area | Purpose |
|---|---|
| `kernel/trusted-dsdt.asl` (move from `mkosi/base/acpi-tables/dsdt.asl`) | One canonical measured AML source |
| `kernel/patches/` (new, explicit ordered patch set) | Narrow kernel restriction on namespace/table loading |
| `kernel/confidential.config`, `kernel/hardening.config` | Built-in DSDT and alternate-loader requirements |
| `kernel/config-x86_64.snapshot` | Regenerated configuration for the selected kernel; no hand-edited snapshot |
| `mkosi/kernel-builder/mkosi.conf` | Pinned ASL compiler and patch-application tooling |
| `src/commands/kernel.rs`, focused helper under `src/kernel/` if needed | Stage/compile trusted DSDT and apply the checked patch set |
| `src/kernel/config.rs` | Fail-closed resolved-config invariants |
| `src/kernel/manifest.rs`, `src/manifest.rs` | New cache inputs, truthful security/provenance metadata |
| `src/commands/build.rs` | Remove early DSDT override while preserving initrd normalization |
| `.github/workflows/base.yml`, `tests/kernel.rs` | Cache parity, clean builds and reproducibility checks |
| New isolated Linux/QEMU test harness and benign ACPI fixtures | Exercise the actual patched kernel before userspace |
| `README.md`, `docs/THREAT_MODEL.md`, `docs/VERIFYING.md`, `docs/MANIFEST.md`, `docs/KERNEL_CONFIGURATION.md` | Replace obsolete security claims and document verification/rollout |

The exact kernel-source files touched by the patch are to be finalized after the complete step-1 trace on the shipping kernel. This plan commits to the enforcement contract, not an unverified pointer-identity implementation.

## Required validation

| Layer | Cases | Passing evidence |
|---|---|---|
| Builder configuration | Consumer disables trusted DSDT/gate, changes header path, or enables alternate loaders | Build fails before compilation/publication |
| Cache/provenance | Change ASL, patch bytes or order; load an old manifest | Cache misses; new artifacts carry the new inputs |
| Kernel patch integration | Exact upstream source; missing/modified patch context; missing or malformed built-in table | Supported source builds; unsupported/malformed cases fail without fallback |
| Boot selection | Default host DSDT, changed OEM identifiers, equal/higher revision, malformed host primary table | Only the built-in DSDT executes, or boot fails before AML execution |
| Secondary tables | Benign extra SSDT, PSDT, OSDT, duplicate/spoofed DSDT | No untrusted AML namespace entry or marker is executed |
| Runtime loading | Dynamic table-install entrypoints and table-load opcodes exercised by an isolated test fixture | Gate rejects before installation/namespace execution; production loader interfaces stay unavailable |
| Compatibility | CPU/memory variations, PCI discovery, RKE2 boot and required GPU layout | Trusted guest functionality remains operational under the supported topology contract |
| Reproducibility/attestation | Independent clean builds; SNP and TDX launches; wrong kernel/policy artifact | Reproducible approved outputs; correct new reference values; incompatible artifacts rejected |

Use inert marker tables and disposable environments. The test must execute the actual patched kernel; a Rust test mirroring a C conditional is not evidence of boot enforcement. QEMU can establish table-selection behavior without claiming TEE isolation. SNP/TDX hardware tests establish the separate measurement and launch acceptance properties.

## Reuse and boundaries

Reuse the existing checksum verification, fresh extraction, nspawn tools-tree runner, config validation and fingerprint patterns. Keep one trusted ASL source and one patch-application path. Do not add an unrelated policy engine, broaden the IPE PR, or change customer-host resources during this work.

The plan does not fix AUDIT-1, AUDIT-4 or AUDIT-5. Those retain their own acceptance criteria. Landing this change does not certify arbitrary privileged workloads or guest-root isolation.

## Approval and completion criteria

The workflow skill requires plan approval before implementation. Following approval, complete implementation, security review, formatting/lint/tests, documentation and a local commit. Do not push or deploy without authorization. Mark AUDIT-3 addressed in source only after the implementation and negative tests establish the contract; mark deployment closure only for rebuilt and hardware-validated consumer images.
