pub mod compose;
pub mod igvm;
pub mod manifest;
pub mod mkosi;
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
    /// Ubuntu cloud image to start from
    #[arg(long)]
    pub source_image: PathBuf,

    /// Output directory for the base partition image
    #[arg(short, long)]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct CloudInitArgs {
    /// Path to cloud-init configuration directory
    pub dir: PathBuf,

    /// Path to hardened kernel
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to base initrd (input to UKI build)
    #[arg(long)]
    pub initrd: PathBuf,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `lunal-build base`)
    #[arg(long)]
    pub base_image: PathBuf,

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
pub struct ContainerArgs {
    /// OCI container image URL
    pub url: String,

    /// Path to hardened kernel
    #[arg(long)]
    pub kernel: PathBuf,

    /// Path to base initrd (input to UKI build)
    #[arg(long)]
    pub initrd: PathBuf,

    /// Path to OVMF firmware binary
    #[arg(long)]
    pub firmware: PathBuf,

    /// Path to base image (from `lunal-build base`)
    #[arg(long)]
    pub base_image: PathBuf,

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
}
