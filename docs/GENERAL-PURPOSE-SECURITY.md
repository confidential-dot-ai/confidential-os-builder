# General-purpose profile security review

These profiles are opt-in **single-tenant, guest-administered** environments.
They are not the minimal hardened base image and are not a sandbox for mutually
untrusted customers inside one VM. CPU and GPU images share the same
container policy. GPU images additionally include the existing NVIDIA policy.

## What changes in the kernel

The table compares requested fragment settings to the committed base kernel
snapshot. A Kconfig request is not proof of its final value: dependency closure
must be reviewed in each resolved `config-x86_64-*.snapshot` before release.
CI checks both required interfaces and the exclusions below after resolution.

| Change | Purpose | Security consequence / mitigation |
|---|---|---|
| `BPF_SYSCALL=n -> y`, `BPF_JIT=n -> y`, `CGROUP_BPF=y` | runc's cgroup-v2 device controller loads/queries/attaches BPF programs. | Adds BPF verifier, program-loading and JIT attack surface. Privileged runtime compromise can reach it. `BPF_UNPRIV_DEFAULT_OFF=y` starts unprivileged access off; `kernel.unprivileged_bpf_disabled=1` latches it off until reboot. `BPF_JIT_ALWAYS_ON=y` removes the interpreter path; `net.core.bpf_jit_harden=2` hardens JIT use for privileged callers too. Neither eliminates verifier/JIT bugs. |
| `USER_NS=n -> y` | OCI user-namespace compatibility, including userns-remapped containers. Not required by every rootful Docker invocation. | Enables namespace-related kernel paths reachable by applications subject to runtime/LSM policy. This profile does not promise supported rootless Docker or impose a guest-wide namespace creation ban. |
| `CFS_BANDWIDTH=n -> y` | Enforce Docker `--cpus` quotas through cgroup-v2 `cpu.max`. | Also selects `GROUP_SCHED_BANDWIDTH=y`, adding scheduler bandwidth-accounting and throttling paths. Prevents a configured container from monopolizing CPU time; it does not impose a default quota or protect against scheduler vulnerabilities. |
| `MEMCG=n -> y` | Enforce container memory and swap limits; Docker otherwise warns and silently discards them. | Adds kernel memory-accounting, reclaim and cgroup OOM paths, with accounting overhead and additional kernel attack surface. Resolves `SLAB_OBJ_EXT=y` and `CGROUP_WRITEBACK=y`; legacy `MEMCG_V1` stays off. Improves resource isolation between containers but is not a confidentiality boundary. Limits must still be configured per container; the readiness check rejects missing memory, swap, CPU-quota or PID-limit capabilities. |
| `VETH=n -> y`, `BRIDGE=n -> y` | Docker bridge networking. | Adds virtual link and bridge processing/control paths. Administrators can create additional networks. VM separation, not a Docker bridge, is the customer trust boundary. |
| `NETFILTER_ADVANCED=n -> y`, `NF_TABLES=n -> y`; `NF_TABLES_INET`, `NF_TABLES_BRIDGE`, `NFT_CT`, `NFT_NAT`, `NFT_MASQ`, `NFT_REDIR`, `NFT_COMPAT`, `NETFILTER_XT_NAT`, `BRIDGE_NETFILTER`, `NF_CONNTRACK_BRIDGE`, `NETFILTER_XT_TARGET_REDIRECT`, `NETFILTER_XT_MATCH_COMMENT`, `NETFILTER_XT_MATCH_MULTIPORT`, `NETFILTER_XT_MATCH_IPRANGE` enabled | nft-backed iptables rules, NAT, published ports and bridge filtering. | More packet parsers and privileged rule-management paths. These are not an ingress or egress policy. Keep Docker's daemon socket private; protect exposed application services. `NETFILTER_XTABLES_LEGACY` remains disabled. |
| `IO_URING=n -> y` | Async-I/O application compatibility, **not needed merely to start Docker**. | Removes the base's compile-time exclusion of an unprivileged syscall family. Docker's default seccomp policy remains applicable but is not a guest-wide fence, and guest root can opt out. Kernel lockdown does not remove this attack surface. This is an explicit compatibility tradeoff requiring approval. |
| GPU only: `MODULES=n -> y`, signature enforcement/keyring and ECC/ECDH/ECDSA/KPP | Existing `gpu.config` policy: signed NVIDIA open modules and CC/SPDM. | Adds module-loader and GPU-driver attack surface. Signatures are mandatory; unloading stays off. GPU boot services load the baked driver and latch further module loads off. CPU workload kernels keep modules disabled. Private signing keys never belong in the image. |
| GPU only: `VSOCKETS=y`, `VIRTIO_VSOCKETS=y` | Existing GPU attestation uses host QGS over vsock. | Introduces an untrusted host/guest communication channel. sshd disallows this address family, but ordinary guest processes are not globally fenced. CPU workload kernels retain no vsock and the `attest` profile can use the ConfigFS TSM path. |

The shared fragment also restates `NETFILTER`, `NETFILTER_XTABLES`,
`NF_CONNTRACK`, `NF_NAT` and `IP_NF_IPTABLES`, already enabled in the base
snapshot. They are not additional off-to-on relaxations. Kconfig can select
further dependencies; the resolved snapshots, not just this table, are the
release-review record. The resolved configurations are recorded in the
[CPU snapshot](../kernel/config-x86_64-general-purpose.snapshot) and
[GPU snapshot](../kernel/config-x86_64-general-purpose-gpu.snapshot).
Their shared dependency closure also enables `BPF_EVENTS`, `NETFILTER_BPF_LINK`,
`IO_WQ`, `IO_URING_ZCRX`, `NET_DEVMEM`, `NET_CRC32C`, `NET_SOCK_MSG`,
`PAGE_POOL`, `STP`, `LLC` and bridge IGMP snooping, plus IPv4/IPv6 nftables
and SCTP/UDPLite conntrack parsing. These are additional kernel paths, not
independent security protections. In particular, io_uring's zero-copy receive
support and BPF tracing/network hooks must be included in the compatibility
tradeoff review; enabling the parent options is not limited to runc's device
controller or ordinary async file I/O. New symbols left disabled by Kconfig
are also visible in the snapshot diffs. These resolved configurations still
require approval before release.

## PoC relaxations deliberately not carried forward

The earlier compatibility fragment enabled interfaces without an ordinary
Docker/vLLM requirement. The new production candidate explicitly keeps:

| Retained protection | Compatibility excluded |
|---|---|
| `MSEAL_SYSTEM_MAPPINGS=y` | CRIU/system-mapping relocation; gVisor/rr and other runtimes that require it need a separately reviewed policy. Sealing blocks changes to kernel-provided userspace mappings such as vDSO/vvar. |
| `CHECKPOINT_RESTORE=n`, `KCMP=n` | CRIU checkpoint/restore and the kcmp resource-sharing oracle. Ordinary container start/stop and disk reattachment do not require CRIU. |
| `USERFAULTFD=n` | Application-managed page-fault handling and its extra kernel surface. |
| `FUSE_FS=n`, `TUN=n` | FUSE filesystems and in-guest TUN/TAP VPNs. Docker's overlayfs/veth bridge does not need either. `VIRTIO_FS` and 9P remain off. |

This candidate therefore differs from the broader PoC kernel and needs fresh
runtime validation. Prior customer success is not acceptance of these bytes.

## What changes in userspace

| Change | Ramification |
|---|---|
| Writable overlays for `/boot`, `/etc`, `/opt`, `/usr` | apt/pip and guest-root administration work. The measured, verity-protected lower filesystem stays intact, but runtime code/configuration can shadow it. Launch attestation does **not** assert current applications, service units, SSH policy or package versions. `/boot` changes alone do not replace the launch stack's measured boot artifacts; administrators must reprovision to update the kernel/image. |
| Rootful Docker/containerd/runc installed and enabled | Docker socket access is guest-root-equivalent. Do not give untrusted users that access or expose an unauthenticated TCP daemon. Containers share the guest kernel and are not separate confidential domains. Default OCI seccomp/capability restrictions are not disabled by this profile. No claim is made of an additional AppArmor/SELinux container policy. |
| Package metadata, Git, Python, compilers and CUDA toolchain (GPU only) retained | More binaries and dependencies to patch, plus intentional arbitrary runtime software installation. Builds/downloads must be trusted by the guest administrator; their bytes are not covered by the boot image's manifest. |
| Docker bridge, NAT and published ports; networkd leaves Docker links unmanaged | No egress allowlist or inbound application firewall is added. No existing base allowlist is removed. Host/KubeVirt routing still controls reachability. TLS, API authentication and exposure policy remain operator/application responsibilities. |
| `user-data` ext4 disk mounted by label; Docker/containerd roots moved there | Data persists only if the disk/PVC is retained. The profile provides **no encryption or integrity protection** for this disk; an untrusted host can read or modify layers, weights and configuration. Recovery and attestation do not authenticate its contents. Use separately designed storage protection for secrets or sensitive data. |
| Operator-key SSH plus root public-key login, ephemeral host keys | Administrators can change SSH configuration after boot. Password and keyboard-interactive authentication start disabled; no serial autologin is added. Verify host keys after replacement. Attested operator-key binding identifies the launch bundle, not the current mutable SSH daemon policy. |

All ephemeral overlays share a 2 GiB tmpfs unless an encrypted ephemeral
scratch disk is supplied. Runtime installs do not survive a reboot. Persistent
storage is an independent launch-stack responsibility, not a property of the
image or a substitute for encrypted scratch.

## Protections not removed

CPU TEE isolation, measured boot, dm-verity for the lower root, mandatory
trusted AML and alternate-AML-loader exclusions remain. So do confidentiality
lockdown, stack protectors, hardened usercopy, allocator randomization/zeroing,
RANDSTRUCT, FORTIFY, UBSAN/KFENCE, kernel/stack ASLR, seccomp, Yama/Landlock,
IOMMU policy, panic-on-oops and the existing KSPP sysctls. Raw memory/port/MSR
access, kexec, hibernation, host-shared filesystems and serial-root autologin
are not enabled. Exact resolved enforcement must still pass the builder's
invariant checks; these are not claims that guest-root mutable policies cannot
be changed after boot.

## Approval and release gates

1. Review resolved CPU/GPU configs and their dependency deltas against the
   base snapshot. Approve BPF, user namespaces, networking and io_uring explicitly.
2. Locally boot each exact candidate; validate SSH, CPU attestation against
   its manifest, mounted disk, apt/Git/PyPI, Docker bridge DNS/HTTPS and port
   publishing. Do not publish based on static tests alone.
3. GPU candidate: validate signed-driver load/latch, CC ON/Ready, encrypted
   multi-GPU mode, GPU attestation, CDI containers and NCCL at each supported
   allocation size. Validate the serving workload separately.
4. Recreate a test VM with its retained test disk and repeat acceptance; do
   not reuse or modify a customer disk. Record the exact image/manifest digests.
5. Publish accepted CPU/GPU artifacts by immutable digest. Reproducibility
   comparison is additional supply-chain evidence, not hardware acceptance.

References: [Linux BPF/JIT](https://docs.kernel.org/networking/filter.html),
[BPF sysctl semantics](https://docs.kernel.org/admin-guide/sysctl/kernel.html),
[system-mapping sealing](https://docs.kernel.org/userspace-api/mseal.html),
[Docker security](https://docs.docker.com/engine/security/),
[Docker seccomp](https://docs.docker.com/engine/security/seccomp/),
[confos threat model](THREAT_MODEL.md).
