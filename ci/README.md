# Image recipes

[`images.json`](images.json) names the image combinations built by CI and the
local [`bin/build-image`](../bin/build-image) wrapper. It is a build registry,
not a VM deployment specification or an inventory of machines.

A **profile** is a composable mkosi configuration directory. A **recipe** chooses
an ordered list of profiles plus the kernel and measurement target needed to
produce one named image. Recipe names become `output/<name>/` directories and
the image-name portion of the GHCR tags.

## Fields

| Field | Meaning |
|---|---|
| Top-level key | Recipe name, such as `general-purpose-gpu`. |
| `profiles` | Ordered mkosi profiles passed to `confos build`; `[]` means no optional profiles. |
| `kernel` | Kernel fragment applied in addition to the baseline, or `null` for the baseline alone. |
| `extra_kernel_inputs` | Additional paths that make the CI selector rebuild this recipe when changed. This is change-detection metadata, not another list of fragments to merge. |
| `platform` | TEE target passed to `confos build`: `snp` for AMD SEV-SNP or `tdx` for Intel TDX. Selects the build/measurement path, not a host allocation. |
| `memory` | Build-time VM memory parameter used by the measurement configuration. It does not allocate RAM, set GPU count, or promise a supported deployment topology. |

The `base` recipe preserves the existing builder defaults: **SNP, 2G, no
optional profiles**. It does not mean these new container images target SNP.
Both `general-purpose` and `general-purpose-gpu` target **TDX**, with build-time
memory parameters of 8G and 128G respectively. Runtime layouts need separate
hardware acceptance and the matching manifest; do not infer allocation from
these values.

## Where this plugs in

1. [`bin/ci-images`](../bin/ci-images) compares Git paths with this registry and
   prints a JSON list of affected recipe names. The general-purpose workflow
   turns it into a two-leg build matrix; the base workflows select their own
   build when `base` is present. It never builds or publishes anything.
2. [`bin/build-image`](../bin/build-image) builds a selected recipe using the
   existing kernel, GPU-staging and image-builder commands. It centralizes the
   flags so local builds and general-purpose CI builds cannot drift apart.
   GPU recipes request `kernel --ensure-tools`: a compiled-kernel cache hit
   still prepares the independently cached toolchain needed for GPU module
   staging. CPU cache hits do not require this extra preparation.
   The existing base workflow keeps its own build orchestration.
3. CI compares independently assembled outputs before any publication. Launch,
   storage attachment, SSH key delivery and GPU assignment are separate tasks.

```sh
bin/ci-images --base origin/main --omit-base
bin/build-image general-purpose
bin/build-image general-purpose-gpu
```

GPU builds require `MODULE_SIG_KEY` or `MODULE_SIG_KEY_PEM`, matching the public
certificate in `kernel/module-signing.crt`. A local public certificate can be
selected with `--module-signing-cert`; it changes the kernel and measurements.
Private keys never belong in this registry or image.

See [selection and caches](../docs/IMAGE-CI.md),
[image operations](../docs/GENERAL-PURPOSE.md), and
[baseline kernel security](../docs/KERNEL-SECURITY.md).
