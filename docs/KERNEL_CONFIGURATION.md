# Linux Kernel configuration

Configuring the kernel that runs inside our Confidential VMs is very important
-- we don't want to open ourselves up to any more security risks than we have
to. That said, we also need our VMs to be useful runners for
[c8s](https://confidential.ai/docs/c8s), our system for running confidential and
attested workloads on Kubernetes.

Our kernel configuration (fragments plus the pinned version) lives in the
`./kernel` directory; the build itself runs inside a `mkosi` tools tree defined
in `mkosi/kernel-builder/`, so the toolchain used to compile the kernel is
reproducibly installed and isolated from the host.

Below, you can find a list of every suggestion from the [Kernel Self-Protection
Program's Recommended Settings
page](https://kspp.github.io/Recommended_Settings) (as of 2026-07-12) that we do
**not** apply, as well as what each setting controls, the security risks
involved in that setting, and an explanation of why we don't apply it.

For this analysis, we used the config `kernel/config-x86_64.snapshot`, used to
compile Linux 6.16.12, on x86_64, with GCC 15, by `mkosi/kernel-builder`. The
kernel command line used in the CVM image is located in `mkosi/base/mkosi.conf`,
and the sysctls applied inside our CVMs are located at
`mkosi/base/mkosi.extra/etc/sysctl.d/99-kspp-hardening.conf`.

Everything on the page not listed below **is** applied, via
`kernel/hardening.config` (which carries its own KSPP annotations),
`x86_64_defconfig` defaults, the sysctl drop-in, or the image cmdline.

---

## Deliberate changes

### `CONFIG_PROC_MEM_NO_FORCE=y` (and cmdline `proc_mem.force_override=never`)

**What it controls.** How `/proc/$pid/mem` honors `FOLL_FORCE` — the flag that
lets a writer punch through read-only and copy-on-write memory mappings of
another (or its own) process. `NO_FORCE` refuses forced writes for every opener;
the `proc_mem.force_override=never` boot parameter pins the same policy at
runtime.

**Risk of not applying.** A process holding a `/proc/$pid/mem` file descriptor
that passed the one-time ptrace access check at `open()` can later modify
read-only program text of the target. Exploits use this to bypass W^X-style
userspace protections and to persist code changes in running processes.

**Why we deviate.** We set `CONFIG_PROC_MEM_FORCE_PTRACE=y` instead: forced
writes are only honored when the opener is *currently ptrace-attached* to the
target, not for any stale fd holder. That closes the fd abuse opening while
keeping real debuggers working, since an attached gdb must be able to plant
breakpoints in read-only text. `NO_FORCE` would break breakpoint insertion.

### `CONFIG_INTEL_IOMMU=y`, `CONFIG_INTEL_IOMMU_DEFAULT_ON=y`, `CONFIG_INTEL_IOMMU_SVM=y`

**What it controls.** The Intel VT-d DMAR driver: hardware IOMMU support on
Intel platforms, whether it is enabled by default without a cmdline flag, and
its Shared Virtual Memory extension.

**Risk of not applying.** On bare metal with Intel hardware, no IOMMU driver
means DMA-capable devices can read/write arbitrary physical memory — the classic
malicious-peripheral / DMA-attack scenario that IOMMU-enforced translation
prevents.

**Why we deviate.** steep kernels only ever run as VM guests, and steep guests
are given no emulated Intel vIOMMU — the DMAR driver would be dead code. 

### `CONFIG_HW_RANDOM_TPM=y`

**What it controls.** Feeding the kernel RNG entropy from a TPM's hardware
random number generator, if a TPM is present.

**Risk of not applying.** One fewer independent entropy source. On systems with
weak or slow entropy at early boot, that can mean predictable random numbers
(keys, ASLR seeds) until the pool fills.

**Why we deviate.** steep exposes no vTPM to the guest at all (`CONFIG_TCG_TPM`
is off), so there is no TPM RNG to read.

### sysctl `kernel.yama.ptrace_scope = 3`

**What it controls.** Yama's ptrace policy. Level 3 blocks `PTRACE_ATTACH` for
everyone, including root and a process's own ancestors, irrevocably until
reboot.

**Risk of not applying.** A compromised process that shares a uid with another
process can attach to it, read its memory (credentials, tokens, keys), and
hijack its execution. Lower scopes only restrict, not eliminate, this lateral
movement.

**Why we deviate.** Level 3 would break all debugging and any workload feature
that legitimately uses ptrace, with no opt-out short of a reboot. steep runs
arbitrary customer workloads inside the guest and cannot assume none of them
need ptrace. We compile Yama in (`CONFIG_SECURITY_YAMA=y`, in the LSM stack) so
the `kernel.yama.ptrace_scope` sysctl exists, but we ship it at the kernel
default of 0 (classic ptrace permissions) — workloads that want ancestor-only
attach can raise it to 1 at runtime. Documented in the header of
`mkosi/base/mkosi.extra/etc/sysctl.d/99-kspp-hardening.conf`.

### sysctl `user.max_user_namespaces = 0`

**What it controls.** The maximum number of user namespaces; 0 disables creating
them entirely.

**Risk of not applying.** Unprivileged user namespaces let any process acquire
(namespaced) `CAP_SYS_ADMIN` and reach kernel interfaces that were historically
root-only — a large attack-surface expansion that has featured in many
privilege-escalation chains.

**Why we deviate.** Container and sandbox tooling in steep workloads depends on
unprivileged user namespaces; setting 0 breaks them. The surrounding mitigations
shrink what a namespaced attacker can reach: the syscall surface is already
heavily cut at compile time (no bpf(2) for unprivileged use —
`CONFIG_BPF_SYSCALL` is off entirely — no io_uring, no userfaultfd, no 32-bit
compat), which removes the interfaces user-namespace escalation chains most
commonly pivot through. Documented in the sysctl drop-in header.

### cmdline `pti=on`

**What it controls.** Forces Kernel Page Table Isolation on even when the CPU
reports it is not vulnerable to Meltdown. We build the mechanism in
(`CONFIG_MITIGATION_PAGE_TABLE_ISOLATION=y`) but leave runtime enablement to the
kernel's CPU-based auto-detection.

**Risk of not applying.** If a CPU falsely claims immunity (or an unknown
Meltdown-class variant appears on "safe" silicon), auto mode leaves the kernel's
page tables mapped while userspace runs, re-opening kernel-memory-read side
channels.

**Why we deviate.** steep guests run exclusively on SEV-SNP (AMD Zen 3+) and TDX
(recent Intel) hosts — parts that were never Meltdown-vulnerable and report
`RDCL_NO`/equivalent. Forcing PTI there is pure overhead (extra TLB flushes on
every kernel entry/exit) with no modeled benefit; and on any genuinely
vulnerable CPU, auto mode still turns PTI on. The mechanism stays compiled in,
so the decision can still be based on the CPU's reported vulnerability.

### cmdline `nosmt` / `mitigations=auto,nosmt`

**What it controls.** Disables Simultaneous Multi-Threading (Hyper-Threading)
sibling CPUs, or lets the mitigation code disable SMT when a mitigation (e.g.
for L1TF/MDS) requires it.

**Risk of not applying.** Cross-hyperthread side channels: a sibling thread can
leak data through shared core resources (L1TF, MDS, and successors) from the
victim thread.

**Why we deviate.** In a guest, SMT topology and physical core scheduling are
controlled by the host VMM, not the guest kernel — a guest-side `nosmt` only
offlines vCPUs the host chose to expose and does not stop the host from
co-scheduling other tenants on the physical sibling. The cross-tenant SMT threat
is addressed at the platform layer (SNP/TDX hosts and their scheduling policy).
KSPP itself flags this as a heavy performance trade-off; the comment block in
`kernel/hardening.config` classifies `nosmt` as a perf/policy opt-in,
deliberately left to deployment configuration. Guest `mitigations=` stays at its
default (`auto`), so all non-SMT CPU mitigations remain active.

### cmdline `slub_debug=ZF`

**What it controls.** SLUB allocator red-zoning (`Z`: guard bytes around objects
to detect overflows) and sanity checks on free (`F`: consistency checking of
freelist metadata).

**Risk of not applying.** Small heap overflows and some use-after-free /
double-free corruption go undetected at the allocator level instead of being
caught and reported at the moment of corruption.

**Why we deviate.** It's slow on every allocation and free, in an allocator
already carrying our hardening choices. On kernels before 6.17 (ours is 6.16),
enabling `slub_debug` **disables kernel pointer hashing**, un-hiding kernel
addresses in every `%p` print — a net security regression for a production image
that sets `kernel.kptr_restrict=2` precisely to hide them (the
`hash_pointers=always` escape hatch only exists from 6.17). And the detection
niche is largely covered at lower cost: `CONFIG_SLAB_FREELIST_HARDENED` catches
freelist metadata corruption, `CONFIG_INIT_ON_FREE_DEFAULT_ON` wipes freed
objects, and KFENCE (`CONFIG_KFENCE=y`, sample interval 100) provides sampled
guard-page overflow/UAF detection with near-zero overhead. 

---

## Inapplicable settings on Linux 6.16.12 / x86 / GCC 15

We do not apply any KSPP recommendations for ARM or other non-x86 architectures.
The recommendations listed below are not applied because the Kconfig symbol no
longer exists (the KSPP page spans many kernel versions), does not exist yet, or
requires Clang. Where the protection still matters, the successor mechanism we
use instead is noted.

### `CONFIG_DEBUG_CREDENTIALS=y`

**What it controlled.** Extra validation of `struct cred` reference counting and
usage, catching credential-structure corruption (a common privilege-escalation
target).

**Why not applicable.** Removed upstream in v6.7 after `struct cred` reference
counting moved to hardened primitives; the symbol does not exist in 6.16. The
underlying overflow class is covered by the always-on `refcount_t` saturation
semantics (see `REFCOUNT_FULL` below).

### `CONFIG_REFCOUNT_FULL=y`

**What it controlled.** Full-precision reference-count overflow/underflow
checking on `refcount_t`, preventing use-after-free via refcount overflow.

**Why not applicable.** Removed in v5.5, when the fast saturation-based checks
became the unconditional `refcount_t` implementation. Every 6.16 kernel has this
protection; there is nothing to enable.

### `CONFIG_PAGE_POISONING_ZERO=y` (and cmdline `page_poison=1`, `slub_debug=P`)

**What it controlled.** Poisoning (with zeros) of freed page allocations, and
the boot parameters that enabled page/slab free-poisoning on pre-5.3 kernels —
limiting the lifetime of stale data and making use-after-free exploitation
harder.

**Why not applicable.** `PAGE_POISONING_ZERO` was removed in v5.11. The KSPP
page itself marks this trio as the legacy path, superseded by
`CONFIG_INIT_ON_ALLOC_DEFAULT_ON=y` and `CONFIG_INIT_ON_FREE_DEFAULT_ON=y` —
both of which we set, wiping slab and page allocations on both allocation and
free.

### `CONFIG_UBSAN_SANITIZE_ALL=y`

**What it controlled.** Applying UBSAN instrumentation to the whole kernel tree
rather than only files that opted in.

**Why not applicable.** Removed in v6.9 (commit 918327e9b7ff): with
`CONFIG_UBSAN=y`, whole-kernel instrumentation is now the unconditional
behavior. We set `CONFIG_UBSAN=y` + `CONFIG_UBSAN_TRAP=y` +
`CONFIG_UBSAN_BOUNDS=y` (with `UBSAN_BOUNDS_STRICT=y` in the resolved config),
so the recommendation's effect is fully in place.

### `CONFIG_RANDOM_TRUST_BOOTLOADER=y`, `CONFIG_RANDOM_TRUST_CPU=y`

**What they controlled.** Crediting entropy from the bootloader-supplied seed
and the CPU's hardware RNG (RDRAND) to the kernel entropy pool.

**Why not applicable.** Both symbols were removed in v6.2; trusting these
sources is now the default behavior (opt out at boot with `random.trust_cpu=off`
/ `random.trust_bootloader=off`, which we do not). The recommended state is what
6.16 always does.

### `CONFIG_GCC_PLUGIN_STRUCTLEAK=y`, `CONFIG_GCC_PLUGIN_STRUCTLEAK_BYREF_ALL=y`

**What they controlled.** A GCC plugin forcing initialization of stack variables
passed by reference, preventing uninitialized-stack-data leaks to userspace —
the pre-GCC-12 route to stack initialization.

**Why not applicable.** The structleak plugin was removed upstream (the symbols
do not exist in 6.16's stack-init choice) because compilers grew native support:
GCC 12+/Clang `-ftrivial-auto-var-init=zero`. We build with GCC 15 and set the
successor, `CONFIG_INIT_STACK_ALL_ZERO=y`, which zero-initializes *all* stack
variables at function entry — a strict superset of what structleak did. KSPP's
own annotation says structleak is only for "earlier GCC".

### `CONFIG_AMD_IOMMU_V2=y`

**What it controlled.** The separate AMD IOMMU v2 driver (GPU-oriented PASID/ATS
features).

**Why not applicable.** The `amd_iommu_v2` driver was removed upstream (v6.7);
its functionality folded into the core driver. We set `CONFIG_AMD_IOMMU=y`,
which is the whole recommendation on 6.16.

(`CONFIG_HARDENED_USERCOPY_DEFAULT_ON=y`, which appeared in this section when
this document covered 6.12, was added upstream in v6.15 and **is** applied since
the 6.16 upgrade — it is pinned in `kernel/hardening.config` next to
`CONFIG_HARDENED_USERCOPY=y`.)

### `CONFIG_CFI_CLANG=y`, `CONFIG_CFI_PERMISSIVE=n`,
`CONFIG_CFI_AUTO_DEFAULT=n`, cmdline `cfi=kcfi`, `CONFIG_UBSAN_LOCAL_BOUNDS=y`

**What they control.** Clang's kernel Control Flow Integrity (KCFI):
type-checking every indirect call to kill forward-edge ROP/JOP, in enforcing
(non-permissive) KCFI mode, as well as Clang's local-variable bounds checking.

**Risk of not applying.** Without software CFI, an attacker with a memory
corruption primitive can more easily pivot indirect calls to arbitrary gadgets.

**Why not applicable.** All of these are Clang-only; steep builds with GCC 15
(required for the GCC hardening plugins we do use: stackleak, latent-entropy,
randstruct-full). Forward-edge protection comes instead from
`CONFIG_X86_KERNEL_IBT=y` (CET Indirect Branch Tracking, enforced in hardware on
the CPUs steep targets) plus `CONFIG_MITIGATION_SLS=y`, ROP surface reduction
via `CONFIG_ZERO_CALL_USED_REGS=y`, and KASLR
(`RANDOMIZE_BASE`/`RANDOMIZE_MEMORY`).

### `CONFIG_KSTACK_ERASE=y` (+ `KSTACK_ERASE_METRICS`/`_RUNTIME_DISABLE` off)
and cmdline `hash_pointers=always`

**What they control.** `KSTACK_ERASE` is the v6.17+ rename of the stackleak
mechanism (wiping the kernel stack on syscall exit); `hash_pointers=always`
(v6.17+) keeps `%p` pointer hashing even when debug options would disable it.

**Why not applicable.** Both exist only from v6.17; we run 6.16. The KSPP page
lists the pre-6.17 equivalents for our case, and we apply them:
`CONFIG_GCC_PLUGIN_STACKLEAK=y` with `STACKLEAK_METRICS` and
`STACKLEAK_RUNTIME_DISABLE` off. Pointer hashing is on by default in 6.16 and
nothing in our config disables it (see the `slub_debug=ZF` entry above).

### Removed "keep this off" symbols: `CONFIG_SECURITY_WRITABLE_HOOKS`,
`CONFIG_SECURITY_SELINUX_DISABLE`, `CONFIG_ACPI_CUSTOM_METHOD`,
`CONFIG_DEVKMEM`, `CONFIG_INET_DIAG` (pre-4.1 note), `CONFIG_X86_X32`,
`CONFIG_HARDENED_USERCOPY_FALLBACK`, `CONFIG_HARDENED_USERCOPY_PAGESPAN`

**What they controlled.** Runtime-disableable SELinux / writable LSM hooks (both
removed in v6.4 — hooks are const now), the ACPI method-injection debugfs
interface (removed v6.9), the `/dev/kmem` kernel-memory device (removed v5.13),
the pre-4.1 `inet_diag` heap-attack concern, the old name of the x32 ABI
option, and the hardened-usercopy fallback / page-span checks (removed upstream
in v5.16 / v6.x).

**Why not applicable / satisfied.** These are "make sure it's off"
recommendations for symbols that either no longer exist in 6.16 (off by
construction) or are off in our config anyway (`CONFIG_INET_DIAG` and
`CONFIG_X86_X32_ABI` are not set).

---

## Removed entirely

Strictly speaking we "do not apply" these as written, because we remove the
entire feature the recommendation tries to constrain. The risk each one
addresses is closed more completely than the recommendation itself would.

| KSPP recommendation | What it would control / risk | What we do instead |
|---|---|---|
| `CONFIG_STRICT_DEVMEM=y`, `CONFIG_IO_STRICT_DEVMEM=y` | Restrict `/dev/mem` to non-kernel, non-driver memory; unrestricted `/dev/mem` lets root read/write all physical memory | `CONFIG_DEVMEM` is not set — `/dev/mem` does not exist at all (KSPP's own first preference) |
| `CONFIG_STRICT_MODULE_RWX=y`, `CONFIG_MODULE_SIG*` (signing, SHA-512, force), `CONFIG_MODULE_FORCE_LOAD=n` | Enforce W^X on module memory and cryptographic signatures on every loaded module; unsigned loading = trivial kernel-code injection for root | `CONFIG_MODULES` is not set — no module loading exists; all code is built in and covered by `STRICT_KERNEL_RWX` (KSPP lists the signing block as the fallback "if CONFIG_MODULES=y is needed") |
| sysctl `kernel.modules_disabled = 1` | Runtime one-way switch to stop module loading | No modules at compile time; the sysctl key does not exist in this kernel |
| sysctl `kernel.kexec_load_disabled = 1` | Block kexec-based replacement of the running kernel | `CONFIG_KEXEC` is not set; kexec does not exist |
| sysctls `kernel.unprivileged_bpf_disabled = 1`, `net.core.bpf_jit_harden = 2` | Keep unprivileged users away from bpf(2); blind the JIT against constant-spray attacks | `CONFIG_BPF_SYSCALL` and `CONFIG_BPF_JIT` are off — there is no bpf(2) and no JIT for any user |
| sysctl `vm.unprivileged_userfaultfd = 0` | Stop unprivileged use of userfaultfd (used to win kernel race conditions) | `CONFIG_USERFAULTFD` is not set |
| sysctls `dev.tty.ldisc_autoload = 0`, `dev.tty.legacy_tiocsti = 0` | Block line-discipline autoload and TIOCSTI keystroke injection | `CONFIG_LDISC_AUTOLOAD` and `CONFIG_LEGACY_TIOCSTI` are not set; the sysctls' compiled-in defaults are already 0 (noted in the sysctl drop-in header) |
| cmdline `vdso32=0` | Ensure the 32-bit vDSO (fixed-address ROP target) stays off | `CONFIG_COMPAT` / `CONFIG_IA32_EMULATION` are not set — no 32-bit vDSO is built |
| cmdline `hardened_usercopy=1`, `init_on_alloc=1`, `init_on_free=1`, `randomize_kstack_offset=on`, `slab_nomerge`, `iommu.passthrough=0 iommu.strict=1`, `kfence.sample_interval=100`, `vsyscall=none`, `proc_mem.force_override` | Runtime switches for their compile-time twins | Each is the boot-parameter form of a CONFIG we set (`HARDENED_USERCOPY`, `INIT_ON_{ALLOC,FREE}_DEFAULT_ON`, `RANDOMIZE_KSTACK_OFFSET_DEFAULT`, `SLAB_MERGE_DEFAULT=n`, `IOMMU_DEFAULT_DMA_STRICT`, `KFENCE_SAMPLE_INTERVAL=100`, `X86_VSYSCALL_EMULATION=n` + `LEGACY_VSYSCALL_NONE`, `PROC_MEM_FORCE_PTRACE`), so the defaults are already the hardened values and the parameters would be no-ops. The one cmdline flag that is *not* redundant, `page_alloc.shuffle=1`, **is** set in `mkosi/base/mkosi.conf` |

---

## Cross-references

- `kernel/hardening.config` — the KSPP block, with per-option comments and its own "intentionally NOT applied" summary.
- `mkosi/base/mkosi.extra/etc/sysctl.d/99-kspp-hardening.conf` — applied sysctls.
- `mkosi/base/mkosi.conf` — image kernel command line (`page_alloc.shuffle=1`).
- `kernel/dev.config` — dev-profile inversions of the hardening baseline (serial console, debugfs); these changes are never shipped in production images.
