# Kernel security baseline

The default image minimizes guest-kernel interfaces exposed to applications
and the untrusted host. Optional profiles deliberately extend that baseline;
their compatibility choices are not changes to the default image's policy.
The CPU TEE protects guest memory from the host, not from a compromised guest
kernel or a trusted guest administrator. See the [threat model](THREAT_MODEL.md).

## Policy sources and enforcement

The baseline combines [`required.config`](../kernel/required.config),
[`hardening.config`](../kernel/hardening.config) and
[`confidential.config`](../kernel/confidential.config) with kernel defaults.
The [resolved baseline snapshot](../kernel/config-x86_64.snapshot) records
Kconfig dependency closure. Image command-line settings live in
[`mkosi.conf`](../mkosi/base/mkosi.conf); runtime KSPP defaults live in
[`99-kspp-hardening.conf`](../mkosi/base/mkosi.extra/etc/sysctl.d/99-kspp-hardening.conf).
Runtime defaults are not all irreversible: trusted guest root can change
mutable policy. A requested fragment value is not proof of the resolved kernel.

| Baseline policy | Security purpose and limitation |
|---|---|
| Measured boot and dm-verity lower root | Bind launch evidence to the kernel and immutable image. They do not attest arbitrary runtime downloads, mutable overlays or attached data disks. |
| Mandatory trusted AML | Admit only the built-in measured DSDT and reject alternate AML-loading paths. Builder invariants prevent profiles from disabling this policy. Non-AML topology and device interfaces remain host-facing attack surface. |
| Confidentiality lockdown; no loadable modules | Restrict kernel modification and sensitive introspection; exclude the module loader entirely. GPU kernels make the signed-module exception documented below. |
| No BPF syscall/JIT, user namespaces or io_uring | Remove substantial application-reachable kernel interfaces. Container-compatible profiles explicitly enable these with documented residual risk. |
| No CRIU/kcmp, userfaultfd, FUSE/TUN or host-shared filesystems | Limit optional process-inspection, fault-handling and host/guest I/O surfaces. Ordinary storage and networking drivers still need review. |
| Stack protection, FORTIFY, hardened usercopy, RANDSTRUCT, allocator hardening, ASLR, UBSAN/KFENCE and system-mapping sealing | Reduce exploit opportunities and detect some memory errors; they do not make kernel vulnerabilities impossible. |
| Seccomp, Yama/Landlock, IOMMU policy, panic-on-oops and KSPP sysctls | Provide additional containment and failure policy. Compiled support does not imply every application uses a sandbox, and panic-on-oops trades availability for fail-stop behavior. |
| Raw memory/port/MSR access, kexec and hibernation excluded | Remove direct kernel-memory and alternate execution/resume paths. Guest root still controls mutable applications and services. |

The [kernel configuration review](KERNEL_CONFIGURATION.md) documents the
trusted-AML enforcement points and each deliberate KSPP exception in detail.
It is the baseline review, not a claim that optional profiles have identical
attack surface. Kernel version changes require re-auditing the trusted-AML
patch and resolved configuration.

## Profile-specific security policy

- [General-purpose CPU](../mkosi/base/mkosi.profiles/general-purpose/README.md):
  exact container-kernel additions, rationale, mitigations, retained exclusions,
  mutable userspace, storage/network limitations and release gates.
- [General-purpose GPU](../mkosi/base/mkosi.profiles/general-purpose-gpu/README.md):
  inherits the CPU policy and documents signed NVIDIA modules, the load latch,
  crypto/vsock interfaces, GPU software and hardware acceptance requirements.

Other compositions must also be assessed against their resolved configurations;
these two READMEs do not approve arbitrary combinations of profiles. In
particular, the `dev` profile's serial autologin is not enabled by either
general-purpose recipe. Build registry semantics are documented in
[`ci/README.md`](../ci/README.md), and
[verification guidance](VERIFYING.md) covers consumer rollout evidence.
