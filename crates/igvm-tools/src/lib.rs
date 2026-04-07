//! IGVM construction and measurement library for AMD SEV-SNP confidential VMs.
//!
//! Provides building (`builder`), measurement (`measure`), and supporting modules
//! for creating IGVM files compatible with QEMU+KVM. The computed launch digest
//! matches hardware attestation when the guest runs on QEMU+KVM.

pub mod builder;
pub mod hob;
pub mod hypervisor;
pub mod manifest;
pub mod measure;
pub mod ovmfmeta;
pub mod x86regs;
