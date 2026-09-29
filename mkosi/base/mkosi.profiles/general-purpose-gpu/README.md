# General-purpose GPU profile

This profile adds CUDA, NCCL, OpenMPI and checksum-pinned `nccl-tests` to the
single-tenant, guest-administered container environment. The recipe composes
`general-purpose`, `gpu`, `attest-gpu` and `ssh` with this profile; selecting
this directory alone is not a complete GPU image.

All [general-purpose security tradeoffs](../general-purpose/README.md) apply:
BPF, user namespaces, io_uring and bridge networking are enabled, guest root
can install software, and persistent `user-data` is neither encrypted nor
authenticated by the profile. Containers share the guest kernel; they are
not separate confidential domains.

## Additional kernel interfaces

The [GPU kernel fragment](../../../../kernel/general-purpose-gpu.config)
combines the shared container policy with the existing `gpu.config` policy.
Review its [resolved snapshot](../../../../kernel/config-x86_64-general-purpose-gpu.snapshot),
not only the fragment, before release.

| Change relative to the CPU image | Purpose | Security consequence / mitigation |
|---|---|---|
| `MODULES=n -> y`, signature enforcement and trusted keyring | Load the baked NVIDIA open kernel modules. | Adds module-loader and GPU-driver attack surface. Signatures are mandatory; module unloading remains disabled. In-tree drivers remain built in. Boot services load the NVIDIA driver, then latch `kernel.modules_disabled=1`, preventing further loads until reboot. No private signing key belongs in the image. |
| ECC/ECDH/ECDSA/KPP crypto interfaces | GPU CC/SPDM session establishment through the Linux Kernel Crypto API. | Adds cryptographic implementation paths required by the driver. Enabling them alone does not prove CC initialization or GPU attestation succeeded. |
| `VSOCKETS=y`, `VIRTIO_VSOCKETS=y` | Fetch TDX quotes from host QGS through vsock. | Adds an untrusted host/guest communication channel. sshd excludes this address family, but ordinary guest processes are not globally fenced and guest root can change userspace policy. Verify quotes externally; the transport is not a trust anchor. |

Mandatory confidentiality lockdown, trusted AML, lower-root dm-verity and
the shared profile's retained hardening still apply. Module signatures allow
the baked driver to load under lockdown; they do not replace the measured
image integrity chain. See [module signing](../../../../docs/module-signing.md).

## Additional userspace and exposure

The image includes NVIDIA driver/CDI tooling, GPU attestation, the CUDA
toolchain, NCCL and MPI. This increases the code and dependencies that must be
maintained. Granting a container CDI GPU access exposes the GPU driver to that
container; container resource limits do not create a separate GPU trust domain.
No inference server, model, API credential or externally listening serving
endpoint is configured by this profile.

GPU counts are detected at runtime. The readiness check requires CC ON and
Ready, rejects devtools mode and requires NVLink encryption for multiple GPUs.
Those diagnostics do not replace cryptographic GPU attestation. Each supported
GPU allocation needs hardware acceptance: signed driver load and latch, CPU/GPU
attestation, CDI containers, NCCL and the intended application. Kernel/image
reproducibility alone cannot establish any of those properties.

See [image operations and acceptance](../../../../docs/GENERAL-PURPOSE.md),
[baseline kernel security](../../../../docs/KERNEL-SECURITY.md), and the
[shared profile release gates](../general-purpose/README.md#approval-and-release-gates).
