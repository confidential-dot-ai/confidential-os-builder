use clap::Parser;
use clap_verbosity_flag::Verbosity;
use steep::{commands, BaseArgs, CloudInitArgs, KernelArgs, RunArgs, SealArgs};

#[derive(Parser)]
#[command(name = "steep", about = "Confidential VM image builder")]
struct Cli {
    #[command(flatten)]
    verbose: Verbosity,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Build the hardened custom kernel (internal)
    #[command(hide = true)]
    Kernel(KernelArgs),
    /// Build the security-hardened base image (internal)
    #[command(hide = true)]
    Base(BaseArgs),
    /// Build a CVM image with cloud-init configuration
    CloudInit(CloudInitArgs),
    /// Seal base image with dm-verity, UKI, and IGVM for measured boot
    Seal(SealArgs),
    /// Launch a confidential VM from build output directory
    Run(RunArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let tracing_level = match cli.verbose.log_level() {
        Some(clap_verbosity_flag::log::Level::Error) => tracing::Level::ERROR,
        Some(clap_verbosity_flag::log::Level::Warn) => tracing::Level::WARN,
        Some(clap_verbosity_flag::log::Level::Info) => tracing::Level::INFO,
        Some(clap_verbosity_flag::log::Level::Debug) => tracing::Level::DEBUG,
        Some(clap_verbosity_flag::log::Level::Trace) => tracing::Level::TRACE,
        None => tracing::Level::ERROR,
    };
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing_level.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match &cli.command {
        Commands::Kernel(args) => commands::kernel::run(args),
        Commands::Base(args) => commands::base::run(args),
        Commands::CloudInit(args) => commands::cloud_init::run(args),
        Commands::Seal(args) => commands::seal::run(args),
        Commands::Run(args) => commands::run::run(args),
    }
}
