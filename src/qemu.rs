use std::path::PathBuf;

use crate::tools;

/// Arguments for launching a CVM with QEMU.
pub struct QemuArgs {
    pub igvm: PathBuf,
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
    pub port_forwards: Vec<(u16, u16)>,
}

impl QemuArgs {
    /// Build the QEMU command-line arguments.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "-machine".to_string(),
            "q35,confidential-guest-support=sev0,igvm-cfg=igvm0".to_string(),
            "-object".to_string(),
            "sev-snp-guest,id=sev0,reduced-phys-bits=1".to_string(),
            "-object".to_string(),
            format!("igvm-cfg,id=igvm0,file={}", self.igvm.display()),
            "-drive".to_string(),
            format!("file={},format={},if=virtio", self.disk.display(), self.disk_format),
            "-smp".to_string(),
            self.smp.to_string(),
            "-m".to_string(),
            self.memory.clone(),
            "-nographic".to_string(),
        ];

        if !self.port_forwards.is_empty() {
            let hostfwds: String = self.port_forwards
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

/// Launch a CVM using QEMU with SEV-SNP.
pub fn launch(args: &QemuArgs) -> anyhow::Result<()> {
    tools::require("qemu-system-x86_64")?;
    let cmd_args = args.to_args();
    tracing::info!(
        igvm = %args.igvm.display(),
        disk = %args.disk.display(),
        smp = args.smp,
        memory = %args.memory,
        "launching CVM via QEMU"
    );
    tools::run_command_streaming("qemu-system-x86_64", &cmd_args)?;
    Ok(())
}
