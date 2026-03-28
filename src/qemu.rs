use std::{os::unix::process::CommandExt as _, path::PathBuf, process::Command};

use crate::tools;

/// The detected QEMU capability tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QemuTier {
    /// Full SEV-SNP + IGVM (confidential computing with measured boot).
    SevSnp,
    /// KVM acceleration only (no confidential computing).
    Kvm,
    /// Pure software emulation (no KVM, no SEV-SNP).
    Emulated,
}

/// Select the best tier from parsed QEMU capabilities.
pub fn select_tier(object_help_output: &str, kvm_available: bool) -> QemuTier {
    let has_sev_snp = object_help_output.contains("sev-snp-guest");
    let has_igvm = object_help_output.contains("igvm-cfg");

    if has_sev_snp && has_igvm && kvm_available {
        QemuTier::SevSnp
    } else if kvm_available {
        QemuTier::Kvm
    } else {
        QemuTier::Emulated
    }
}

/// Detect the best available QEMU tier by probing the system.
pub fn detect_tier() -> anyhow::Result<QemuTier> {
    tools::require("qemu-system-x86_64")?;

    let object_help = tools::run_command("qemu-system-x86_64", &["-object", "help"])?;

    let kvm_available = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok();

    Ok(select_tier(&object_help, kvm_available))
}

/// Arguments for launching a VM with QEMU.
pub struct QemuArgs {
    pub tier: QemuTier,
    pub igvm: Option<PathBuf>,
    pub uki: Option<PathBuf>,
    pub firmware: Option<PathBuf>,
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
    pub port_forwards: Vec<(u16, u16)>,
}

impl QemuArgs {
    /// Build the QEMU command-line arguments.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = match self.tier {
            QemuTier::SevSnp => {
                let igvm = self.igvm.as_ref().expect("SevSnp tier requires igvm");
                vec![
                    "-machine".to_string(),
                    "q35,confidential-guest-support=sev0,igvm-cfg=igvm0".to_string(),
                    "-object".to_string(),
                    "sev-snp-guest,id=sev0,reduced-phys-bits=1".to_string(),
                    "-object".to_string(),
                    format!("igvm-cfg,id=igvm0,file={}", igvm.display()),
                ]
            }
            QemuTier::Kvm => {
                let uki = self.uki.as_ref().expect("Kvm tier requires uki");
                let firmware = self.firmware.as_ref().expect("Kvm tier requires firmware");
                vec![
                    "-machine".to_string(),
                    "q35".to_string(),
                    "-enable-kvm".to_string(),
                    "-drive".to_string(),
                    format!(
                        "if=pflash,format=raw,readonly=on,file={}",
                        firmware.display()
                    ),
                    "-kernel".to_string(),
                    uki.display().to_string(),
                ]
            }
            QemuTier::Emulated => {
                let uki = self.uki.as_ref().expect("Emulated tier requires uki");
                let firmware = self
                    .firmware
                    .as_ref()
                    .expect("Emulated tier requires firmware");
                vec![
                    "-machine".to_string(),
                    "q35".to_string(),
                    "-drive".to_string(),
                    format!(
                        "if=pflash,format=raw,readonly=on,file={}",
                        firmware.display()
                    ),
                    "-kernel".to_string(),
                    uki.display().to_string(),
                ]
            }
        };

        args.push("-drive".to_string());
        args.push(format!(
            "file={},format={},if=virtio",
            self.disk.display(),
            self.disk_format
        ));
        args.push("-smp".to_string());
        args.push(self.smp.to_string());
        args.push("-m".to_string());
        args.push(self.memory.clone());
        args.push("-nographic".to_string());

        if !self.port_forwards.is_empty() {
            let hostfwds: String = self
                .port_forwards
                .iter()
                .map(|(h, g)| format!("hostfwd=tcp::{}-:{}", h, g))
                .collect::<Vec<_>>()
                .join(",");
            args.push("-netdev".to_string());
            args.push(format!("user,id=net0,{}", hostfwds));
            args.push("-device".to_string());
            args.push("virtio-net-pci,netdev=net0".to_string());
        }

        args
    }
}

/// Launch a VM using QEMU.
pub fn launch(args: &QemuArgs) -> anyhow::Result<()> {
    tools::require("qemu-system-x86_64")?;
    let cmd_args = args.to_args();
    match args.tier {
        QemuTier::SevSnp => {
            tracing::info!(
                igvm = %args.igvm.as_ref().unwrap().display(),
                disk = %args.disk.display(),
                smp = args.smp,
                memory = %args.memory,
                "launching CVM via QEMU (SEV-SNP)"
            );
        }
        QemuTier::Kvm | QemuTier::Emulated => {
            tracing::info!(
                uki = %args.uki.as_ref().unwrap().display(),
                firmware = %args.firmware.as_ref().unwrap().display(),
                disk = %args.disk.display(),
                smp = args.smp,
                memory = %args.memory,
                tier = ?args.tier,
                "launching VM via QEMU"
            );
        }
    }

    let _ = Command::new("qemu-system-x86_64").args(&cmd_args).exec();

    Ok(())
}
