use clap::Parser;

#[derive(Parser)]
#[command(name = "lunal-build", about = "Confidential VM image builder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Build the hardened custom kernel
    Kernel,
    /// Build the security-hardened base image
    Base,
    /// Build a CVM image with cloud-init configuration
    CloudInit,
    /// Build a CVM image running a container
    Container,
}

fn main() {
    let _cli = Cli::parse();
    println!("lunal-build");
}
