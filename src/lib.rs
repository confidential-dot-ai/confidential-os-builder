pub mod igvm;
pub mod manifest;
pub mod qemu;
pub mod tools;

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct KernelArgs {
    /// Path to kernel source tree
    #[arg(long)]
    pub source: PathBuf,

    /// Path to kernel .config (hardening config)
    #[arg(long)]
    pub config: PathBuf,

    /// Output directory for kernel + initrd
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct BaseArgs {
    /// Force mkosi to rebuild the image even if it exists
    #[arg(short, long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct CloudInitArgs {
    /// Path to cloud-init configuration directory
    pub dir: PathBuf,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Output directory from steep seal or steep cloud-init
    pub dir: PathBuf,

    /// Forward a host port to a guest port (HOST:GUEST, e.g. 8080:80). Repeatable.
    #[arg(long, value_name = "HOST:GUEST")]
    pub port_forward: Vec<String>,

    /// Path to QEMU binary
    #[arg(long, default_value = "qemu-system-x86_64", env = "STEEP_QEMU_BIN")]
    pub qemu_bin: String,

    /// Path to OVMF firmware (overrides manifest; needed for --skip-igvm images on KVM)
    #[arg(long, env = "STEEP_FIRMWARE")]
    pub firmware: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SealArgs {
    /// Output directory for sealed artifacts (IGVM, UKI, manifest, disk)
    #[arg(short, long, default_value = "output/sealed")]
    pub output: PathBuf,

    /// Path to cloud-init user-data file to include in the image
    #[arg(long)]
    pub cloud_init: Option<PathBuf>,

    /// Pre-apply cloud-init at build time (chroot + cloud-init before verity).
    /// Without this flag, cloud-init runs at boot from the measured config.
    #[arg(long, requires = "cloud_init")]
    pub bake: bool,

    /// Skip IGVM generation (produce only disk + UKI, no SNP measurement)
    #[arg(long)]
    pub skip_igvm: bool,

    /// Path to OVMF firmware binary (required unless --skip-igvm)
    #[arg(long, env = "STEEP_FIRMWARE")]
    pub firmware: Option<PathBuf>,

    /// Path to igvm-tools binary (required unless --skip-igvm)
    #[arg(long, env = "STEEP_IGVM_TOOLS")]
    pub igvm_tools: Option<PathBuf>,

    /// RAM for VM (QEMU-style suffix, e.g. "4G")
    #[arg(long, default_value = "4G")]
    pub memory: String,

    /// Number of vCPUs
    #[arg(long, default_value = "1")]
    pub smp: u32,
}

pub mod commands {
    pub mod base;
    pub mod cloud_init;
    pub mod kernel;
    pub mod run;
    pub mod seal;
}
