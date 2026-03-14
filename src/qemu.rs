use std::path::PathBuf;

use crate::tools;

/// Arguments for launching a CVM with QEMU.
pub struct QemuArgs {
    pub igvm: PathBuf,
    pub disk: PathBuf,
    pub disk_format: String,
    pub smp: u32,
    pub memory: String,
}

impl QemuArgs {
    /// Build the QEMU command-line arguments.
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "-machine".to_string(),
            "q35,confidential-guest-support=sev0,igvm-cfg=igvm0".to_string(),
            "-object".to_string(),
            "sev-snp-guest,id=sev0".to_string(),
            "-object".to_string(),
            format!("igvm-cfg,id=igvm0,file={}", self.igvm.display()),
            "-drive".to_string(),
            format!("file={},format={},if=virtio", self.disk.display(), self.disk_format),
            "-smp".to_string(),
            self.smp.to_string(),
            "-m".to_string(),
            self.memory.clone(),
            "-nographic".to_string(),
        ]
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
