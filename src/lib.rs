pub mod compose;
pub mod container;
pub mod convert;
pub mod igvm;
pub mod manifest;
pub mod mkosi;
pub mod nftables;
pub mod pipeline;
pub mod qemu;

pub mod tools;
pub mod uki;

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

    /// Path to kernel (or prebuilt UKI EFI when --initrd is omitted)
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to initrd. When omitted, --kernel is treated as a prebuilt UKI (ukify step is skipped).
    #[arg(long)]
    pub initrd: Option<PathBuf>,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `steep base`)
    #[arg(long)]
    pub base_image: PathBuf,

    /// RAM for VM (QEMU-style suffix, e.g. "2G")
    #[arg(long, default_value = "2G")]
    pub memory: String,

    /// Number of vCPUs (affects SNP launch digest)
    #[arg(long, default_value = "1")]
    pub smp: u32,

    /// Output image format
    #[arg(long, default_value = "qcow2")]
    pub format: ImageFormat,

    /// Output directory for artifacts
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Output directory from steep cloud-init or steep container
    pub dir: PathBuf,

    /// Forward a host port to a guest port (HOST:GUEST, e.g. 8080:80). Repeatable.
    #[arg(long, value_name = "HOST:GUEST")]
    pub port_forward: Vec<String>,
}

#[derive(clap::Args)]
pub struct ContainerArgs {
    /// OCI container image URL
    pub url: String,

    /// Path to kernel (or prebuilt UKI EFI when --initrd is omitted)
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to initrd. When omitted, --kernel is treated as a prebuilt UKI (ukify step is skipped).
    #[arg(long)]
    pub initrd: Option<PathBuf>,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `steep base`)
    #[arg(long)]
    pub base_image: PathBuf,

    /// Single TCP port to allow through firewall
    #[arg(long)]
    pub service_port: u16,

    /// RAM for VM (QEMU-style suffix, e.g. "2G")
    #[arg(long, default_value = "2G")]
    pub memory: String,

    /// Number of vCPUs (affects SNP launch digest)
    #[arg(long, default_value = "1")]
    pub smp: u32,

    /// Output image format
    #[arg(long, default_value = "qcow2")]
    pub format: ImageFormat,

    /// Output directory for artifacts
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ImageFormat {
    Qcow2,
    Vhd,
    Raw,
}

pub mod commands {
    pub mod base;
    pub mod cloud_init;
    pub mod container;
    pub mod kernel;
    pub mod run;
}
