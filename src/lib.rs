pub mod igvm;
pub mod kernel;
pub mod kernel_cache;
pub mod manifest;
pub mod qemu;
pub mod tools;

use std::path::PathBuf;

#[derive(clap::Args)]
pub struct KernelArgs {
    /// Force rebuild even if cache is current
    #[arg(short, long)]
    pub force: bool,

    /// Regenerate kernel/config-x86_64.snapshot from defconfig + fragments.
    /// Use after bumping the kernel version or editing the fragments.
    #[arg(long)]
    pub update_snapshot: bool,

    /// Output directory.
    #[arg(short, long, default_value = "output/kernel")]
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
pub struct BuildArgs {
    /// Output directory for artifacts (IGVM, UKI, manifest, disk image)
    #[arg(default_value = "base")]
    pub name: PathBuf,

    /// Path to cloud-init user-data file to optionally include in the image
    #[arg(short, long)]
    pub cloud_init: Option<PathBuf>,

    /// Enable debug console (passwordless root autologin on serial).
    /// WARNING: In the SNP threat model, the host controls the serial port.
    /// This changes the image measurement.
    #[arg(long, verbatim_doc_comment)]
    pub console: bool,

    /// Skip IGVM generation (produce only disk + UKI, no SNP measurement)
    #[arg(long)]
    pub skip_igvm: bool,

    /// Path to OVMF firmware binary
    #[arg(long, env = "STEEP_FIRMWARE", default_value = "output/OVMF.fd")]
    pub firmware: PathBuf,

    /// RAM for VM (QEMU-style suffix, e.g. "4G")
    #[arg(long, default_value = "4G")]
    pub memory: String,

    /// Number of vCPUs
    #[arg(long, default_value = "2")]
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
pub struct PushArgs {
    /// Directory to push (output from `steep build`)
    pub dir: PathBuf,

    /// OCI registry (e.g. ghcr.io/lunal-dev)
    #[arg(long, default_value = "ghcr.io/lunal-dev")]
    pub registry: String,

    /// Image name
    #[arg(long)]
    pub name: Option<String>,

    /// Image tag
    #[arg(long, default_value = "latest")]
    pub tag: String,
}

#[derive(clap::Args)]
pub struct PullArgs {
    /// Image name to pull (e.g. "base")
    pub name: String,

    /// OCI registry (e.g. ghcr.io/lunal-dev)
    #[arg(long, default_value = "ghcr.io/lunal-dev")]
    pub registry: String,

    /// Image tag
    #[arg(long, default_value = "latest")]
    pub tag: String,
}

pub mod commands {
    pub mod build;
    pub mod igvm;
    pub mod kernel;
    pub mod pull;
    pub mod push;
    pub mod run;
}
