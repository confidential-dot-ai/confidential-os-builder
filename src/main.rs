use clap::Parser;
use clap_verbosity_flag::Verbosity;
use steep::{
    commands, BaseArgs, CloudInitArgs, ContainerArgs, KernelArgs, RunArgs,
};

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
    /// Build the hardened custom kernel
    Kernel(KernelArgs),
    /// Build the security-hardened base image
    Base(BaseArgs),
    /// Build a CVM image with cloud-init configuration. The cloud-init user-data must configure any required firewall rules (e.g. opening a service port with nftables).
    CloudInit(CloudInitArgs),
    /// Build a CVM image running a container
    Container(ContainerArgs),
    /// Launch a confidential VM from build artifacts
    Run(RunArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(
            match cli.verbose.log_level_filter() {
                clap_verbosity_flag::log::LevelFilter::Off => {
                    tracing_subscriber::filter::LevelFilter::OFF
                }
                clap_verbosity_flag::log::LevelFilter::Error => {
                    tracing_subscriber::filter::LevelFilter::ERROR
                }
                clap_verbosity_flag::log::LevelFilter::Warn => {
                    tracing_subscriber::filter::LevelFilter::WARN
                }
                clap_verbosity_flag::log::LevelFilter::Info => {
                    tracing_subscriber::filter::LevelFilter::INFO
                }
                clap_verbosity_flag::log::LevelFilter::Debug => {
                    tracing_subscriber::filter::LevelFilter::DEBUG
                }
                clap_verbosity_flag::log::LevelFilter::Trace => {
                    tracing_subscriber::filter::LevelFilter::TRACE
                }
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
        Commands::Run(args) => commands::run::run(args),
    }
}
