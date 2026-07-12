# Architecture

A map of the codebase for contributors: what lives where, and how a
`steep build` flows through it. For the *domain* concepts (UKI, dm-verity,
IGVM, measured boot) read [CONCEPTS.md](CONCEPTS.md) first — this document
assumes them.

## Bird's eye view

Steep is a Rust CLI that orchestrates external tools (mkosi, QEMU, iasl,
oras, systemd's ukify/repart machinery via mkosi) plus two in-repo Rust
crates that do the measurement math. The CLI's job is deterministic
assembly; the crates' job is predicting, offline, exactly what the TEE
hardware will measure at launch.

```
bin/steep build
   │
   ├─ 1. kernel        src/commands/kernel.rs + src/kernel/*    (cached)
   ├─ 2. base image    mkosi/base/ via mkosi                    (reproducible rootfs)
   ├─ 3. initrd + DSDT mkosi/initrd/ + iasl early-cpio prepend
   ├─ 4. UKI + verity  mkosi (erofs + verity + ukify)  → disk.raw, uki.efi, roothash
   ├─ 5. SNP measure   src/igvm/ → crates/igvm-tools   → guest-smp<N>.igvm + digests
   ├─ 6. TDX measure   crates/tdx-measure (library)    → mrtd/rtmr1/rtmr2
   └─ 7. manifest      src/manifest.rs                 → manifest.json
```

## Repository layout

### `src/` — the steep CLI

| Path | Role |
|---|---|
| `main.rs` | clap entry point; subcommand dispatch |
| `lib.rs` | Shared argument structs and `BuildPlatform` (snp/tdx/both) |
| `commands/build.rs` | The pipeline above. Per-build file injections (cloud-init, `--extra`, dev-profile console) go into a temporary `mkosi.local/` overlay removed by an RAII guard; the trusted-DSDT step compiles ASL → AML and prepends an uncompressed early cpio to the mkosi initrd, and *that* combined initrd is what the UKI and both measurement paths see |
| `commands/kernel.rs` | Kernel build orchestration + cache check |
| `commands/run.rs` | Boot an output dir in QEMU |
| `commands/igvm.rs` | Re-render IGVM SMP variants for an existing build |
| `commands/push.rs`, `commands/pull.rs` | OCI transfer via `oras` |
| `kernel/` | `version.rs` parses `kernel/version`; `fetch.rs` downloads the pinned tarball (SHA-256-checked); `config.rs` merges fragments and maintains the snapshot lockfile; `compile.rs` drives the mkosi kernel-builder; `manifest.rs` fingerprints inputs for caching |
| `kernel_cache.rs` | Thin cache-aware accessor `commands/build.rs` uses to get a kernel artifact |
| `igvm/invoke.rs` | Bridges to the `igvm-tools` crate to emit IGVMs and capture digests |
| `manifest.rs` | Manifest schema (v3), hashing helpers, version-gated reader — see [MANIFEST.md](MANIFEST.md) |
| `qemu.rs` | QEMU invocation: tier probing (SNP → KVM → emulated), argument construction, scratch-disk attachment (virtio serial `confai-scratch`), memory-string validation |
| `tools.rs` | External-tool discovery and subprocess error handling |

### Kernel configuration model

`kernel/` holds a version pin (`version`: LINUX_VERSION + tarball SHA) and
config fragments merged in order:

```
x86_64_defconfig → required.config → hardening.config → confidential.config → [caller fragment] → mod2yesconfig → olddefconfig
```

Later fragments win, which is how `confidential.config` deliberately
re-enables options `hardening.config` turned off (e.g.
`CONFIG_ACPI_TABLE_UPGRADE` for the trusted-DSDT override). The resolved
`.config` is written to `kernel/config-x86_64.snapshot`, a committed
lockfile — every build rewrites it and `git diff` reveals config drift. The
build fails if `olddefconfig` silently dropped any `=y` a fragment
requested (unmet dependency), rather than shipping a weaker kernel.

### `crates/` — measurement engines

Both are usable as standalone CLIs and are steep's only in-repo library
dependencies. They deliberately have no dependency on steep itself, so
verifiers can `cargo install` just the measurement tool.

- **`igvm-tools`** — builds IGVM files (firmware + UKI + VMSA layout) and
  computes SNP launch digests. QEMU+KVM-specific by design: page ordering
  and VMSA contents replicate that VMM exactly, because the digest only
  matches hardware reports if the simulation matches the VMM. Vendored from
  the original igvm-tools repo, developed in-tree since.
- **`tdx-measure`** — computes MRTD (TDVF simulation), RTMR[1]/RTMR[2] (UKI
  Authenticode + section chain), parses/replays CCEL event logs, and
  verifies live TDREPORTs. Modules: `tdvf`, `rtmr`, `pe`, `ccel`, `esp`.

### `mkosi/` — image definitions

| Dir | Produces |
|---|---|
| `base/` | The Ubuntu guest rootfs → erofs+verity disk and UKI. `mkosi.conf.d/` splits config, `mkosi.extra/` is baked-in filesystem content, `mkosi.profiles/` holds the `dev` (serial autologin), `attest` (attestation-api service, fetched from GHCR by digest), and `ssh` (openssh-server, host keys stripped for reproducibility) profiles, `mkosi.repart/` defines the GPT layout, `acpi-tables/dsdt.asl` is the trusted DSDT source |
| `initrd/` | A minimal custom initrd; `mkosi.extra/init` is the entire early-boot logic — verity root setup, overlay assembly, scratch-disk detection/encryption |
| `kernel-builder/` | A tools-tree image in which the guest kernel is compiled, isolating the toolchain from the host for reproducibility |

Reproducibility work (`SOURCE_DATE_EPOCH`, mtime normalization, etc.) is
documented in [REPRODUCIBILITY.md](REPRODUCIBILITY.md).

### Everything else

- `bin/` — `setup` (host deps), `steep` (cargo-build-and-run wrapper),
  `test` (cargo-nextest), `lint` (rustfmt + clippy). CI runs both, plus a
  `cargo deny check` gate (licenses/advisories) and a release build that
  `test`/`lint` don't cover locally.
- `tests/` — integration tests: CLI surface (`cli.rs`), manifest round-trips
  (`manifest.rs`), kernel config resolution (`kernel.rs`), QEMU argument
  construction and scratch behavior (`qemu.rs`, `qemu_scratch.rs`), tool
  discovery (`tools.rs`), plus `e2e.sh` for full build-and-boot runs that
  need a capable host.
- `.github/workflows/` — `test.yml` (bin/test on Linux x86/arm + macOS;
  bin/lint, cargo-deny, and a build job on Linux); `base.yml` (builds the
  base image on every push to `main` and publishes it to GHCR via oras as
  `ghcr.io/confidential-dot-ai/steep:base`, plus a pinned `base-<short SHA>`
  tag).
- `output/OVMF.fd` — the one committed binary: steep's IGVM-aware OVMF built
  from the [edk2 fork](https://github.com/confidential-dot-ai/edk2). It's
  checked in (despite `output/` being gitignored) so builds work without
  compiling edk2; `crates/igvm-tools/README.md` documents rebuilding it.

## Design principles

Worth internalizing before changing the build pipeline:

1. **Everything that executes in the guest is measured.** Any new way of
   getting content into an image must land in the verity root, the UKI, or
   the IGVM — never in an unmeasured side channel.
2. **Builds are reproducible.** New pipeline steps must be deterministic
   (no timestamps, no randomness, stable file ordering); `build.timestamp`
   in the manifest is the only sanctioned nondeterminism.
3. **Measurement simulations must match the VM memory byte-for-byte.** Changes to
   IGVM layout or UKI assembly usually require matching changes in
   `igvm-tools`/`tdx-measure`, and vice versa. Validate against hardware
   when possible.
4. **The manifest is versioned and strict.** Schema changes bump
   `MANIFEST_VERSION`; readers reject other versions rather than migrate
   (measurements must be recomputed, never translated).
5. **CLI flags and README stay in sync** — enforced by convention (see
   [CONTRIBUTING.md](../CONTRIBUTING.md)).
