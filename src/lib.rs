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

    /// Enable debug console (passwordless root autologin on serial).
    /// WARNING: In the SNP threat model, the host controls the serial port.
    /// This changes the image measurement.
    #[arg(long)]
    pub debug: bool,

    /// Skip IGVM generation (produce only disk + UKI, no SNP measurement)
    #[arg(long)]
    pub skip_igvm: bool,

    /// Path to OVMF firmware binary (required unless --skip-igvm)
    #[arg(long, env = "STEEP_FIRMWARE")]
    pub firmware: Option<PathBuf>,

    /// RAM for VM (QEMU-style suffix, e.g. "4G")
    #[arg(long, default_value = "4G")]
    pub memory: String,

    /// Number of vCPUs
    #[arg(long, default_value = "1")]
    pub smp: u32,
}

#[derive(clap::Args)]
pub struct IgvmArgs {
    /// Sealed output directory (from steep seal)
    pub dir: PathBuf,

    /// SMP counts to generate IGVM files for (e.g. --smp 1 2 4 8)
    #[arg(long, required = true, num_args = 1..)]
    pub smp: Vec<u32>,

    /// Path to OVMF firmware binary
    #[arg(long, env = "STEEP_FIRMWARE")]
    pub firmware: PathBuf,
}

#[derive(clap::Args)]
pub struct PublishArgs {
    /// Sealed output directory (from steep seal)
    pub dir: PathBuf,

    /// OCI registry (e.g. ghcr.io/lunal-dev)
    #[arg(long, default_value = "ghcr.io/lunal-dev")]
    pub registry: String,

    /// Image name
    #[arg(long, default_value = "base-cpu-image")]
    pub name: String,

    /// Image tag (default: sha-<disk hash> from manifest)
    #[arg(long)]
    pub tag: Option<String>,

    /// Push to registry after building
    #[arg(long)]
    pub push: bool,
}

pub mod commands {
    pub mod base;
    pub mod cloud_init;
    pub mod igvm;
    pub mod kernel;
    pub mod publish;
    pub mod run;
    pub mod seal;
}
