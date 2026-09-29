# Image selection and cache policy

`ci/images.json` is the registry of published image recipes: `base`,
`general-purpose` and `general-purpose-gpu`. Profile compositions are explicit;
CI does not guess every possible combination of shipped/external profiles.
`bin/build-image <name>` is the shared local/CI build entrypoint. It never
launches a VM or publishes an image.

`bin/ci-images --base <commit>` classifies additions, modifications and deletions:

- A profile change selects every registered recipe that uses that profile.
  An unused development profile does not rebuild production images.
- A recipe-specific kernel fragment selects its consumers; shared CPU/GPU
  policy changes select both general-purpose images.
- Base/initrd/kernel-tools, shared hardening, kernel pins/patches, builder,
  firmware, recipe registry and shared CI actions select all recipes.
- Documentation-only edits select none. A missing/zero previous Git object
  selects all rather than silently skipping a build. Manual dispatch selects all.

The base push build and PR repro gate use the same classifier. General-purpose
builds use its output as a dynamic matrix, with two build legs per selected
image. This avoids running the base repro gate for a GPU-only profile edit.

The tools stamp is compared byte-for-byte with its written key. Previously,
trimming the stored key removed the empty extra-package field's newline while
the expected key retained it, forcing normal builds to miss even with a
restored tools tree. A regression test covers both empty and populated lists.

## Cache layers

| Layer | Reuse boundary / validation |
|---|---|
| Runner host dependencies | A separate shared action keys the verified offline deb set by runner image, snapshot, installer and requested packages. It saves immediately after successful installation, so a later image failure does not discard it. Cold APT metadata failures stop bootstrap explicitly; signed snapshot sources and package verification stay mandatory. |
| Distro package pool: `mkosi/mkosi.pkgcache` | Explicit `PackageCacheDirectory=../mkosi.pkgcache` in all three mkosi stages. Shared across profiles/stages. apt uses signed indexes and package hashes; no live-mirror fallback is added. A rolling run/variant/leg key lets warm builds save added packages instead of freezing the first pool forever. |
| Repository metadata | Stage-local `mkosi.cache/*.metadata.cache` and keyring cache: kernel tools and image stages have different pinned snapshot dates. Pool restore keys include all snapshot configuration. Incremental image builds stay off; mkosi may still resync metadata. This is not a guarantee of fully offline builds. |
| Kernel sources | One shared `output/kernel/cache` namespace keyed by the pinned kernel version/hash. The downloader rechecks SHA256. |
| Kernel compiler tools | Shared across image profiles using the same tools config, sandbox and builder implementation. The builder verifies its stamp against configuration before reuse. Harness tools with extra QEMU/Python remain a distinct cache. |
| Compiled kernel + resolved snapshot | Lineage/input-specific. `confos` compares its full input fingerprint and hashes vmlinuz before accepting a hit. A cached CPU kernel is never blindly substituted for a GPU kernel. |
| NVIDIA archives | Shared GPU cache, download hashes checked on every use. The driver sources, userspace installer and toolkit remain pinned. |
| Unsigned NVIDIA modules | Content-keyed by build script/helper, kernel manifest, actual kernel config and symbol table. Corrupt/partial entries miss. Cached bytes are copied, then freshly signed with the supplied key; private keys and signed staging trees never enter this cache. `CONFOS_GPU_MODULE_CACHE_DISABLE=1` forces compilation for an independent check. |
| CUDA/NCCL inputs | Shared checksum-pinned downloads, validated again by the sync hook. |

The shared `.github/actions/build-cache` restore/save action is used by base and
general-purpose image workflows. Saves run with `always()` so successful tool,
source, module and package work survives a later image failure. The action never
caches `mkosi.local`, registry credentials, private signing keys, final root
disks or UKIs. Image assembly is repeated for reproducibility evidence.

GitHub caches are **trusted build inputs**, not independently authenticated
artifacts. Hashes detect corruption and bind input identities; they do not
protect against a writer that replaces both cached bytes and their checksum.
Do not restore untrusted fork-generated caches into signing/publishing jobs.
GitHub's branch/default-branch visibility rules still apply: matching keys do
not make sibling-branch caches universally visible. Concurrent saves can race;
one cache wins and missing packages are fetched on the next build, not silently
substituted. Storage/eviction limits may require later cache housekeeping.

Publication is separate from caching. Feature pushes build but do not publish.
Manual `publish=true` dispatch is for accepted candidates; main may update
latest aliases only after the two measured outputs match. Local VM/attestation
acceptance remains an operator release gate, not a claim made by cache hits.
