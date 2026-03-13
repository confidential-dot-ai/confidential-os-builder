mod commands;
mod compose;
mod igvm;
mod manifest;
mod mkosi;
mod tools;
mod uki;

use std::path::PathBuf;

use clap::Parser;
use clap_verbosity_flag::Verbosity;

#[derive(Parser)]
#[command(name = "lunal-build", about = "Confidential VM image builder")]
pub struct Cli {
    #[command(flatten)]
    pub verbose: Verbosity,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    /// Build the hardened custom kernel
    Kernel(KernelArgs),
    /// Build the security-hardened base image
    Base(BaseArgs),
    /// Build a CVM image with cloud-init configuration
    CloudInit(CloudInitArgs),
    /// Build a CVM image running a container
    Container(ContainerArgs),
}

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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(
            match cli.verbose.log_level_filter() {
                clap_verbosity_flag::log::LevelFilter::Off => tracing_subscriber::filter::LevelFilter::OFF,
                clap_verbosity_flag::log::LevelFilter::Error => tracing_subscriber::filter::LevelFilter::ERROR,
                clap_verbosity_flag::log::LevelFilter::Warn => tracing_subscriber::filter::LevelFilter::WARN,
                clap_verbosity_flag::log::LevelFilter::Info => tracing_subscriber::filter::LevelFilter::INFO,
                clap_verbosity_flag::log::LevelFilter::Debug => tracing_subscriber::filter::LevelFilter::DEBUG,
                clap_verbosity_flag::log::LevelFilter::Trace => tracing_subscriber::filter::LevelFilter::TRACE,
            }
            .into(),
        )
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match &cli.command {
        Commands::Kernel(args) => commands::kernel::run(args),
        Commands::Base(args) => commands::base::run(args),
        Commands::CloudInit(args) => commands::cloud_init::run(args),
        Commands::Container(args) => commands::container::run(args),
    }
}
