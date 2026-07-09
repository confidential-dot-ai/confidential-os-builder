pub mod igvm;
pub mod kernel;
pub mod kernel_cache;
pub mod manifest;
pub mod qemu;
pub mod tools;

use std::path::PathBuf;

/// Which confidential-VM platform(s) `steep build` should emit
/// measurements for. The artifact set (disk + UKI + IGVM) is the same
/// across platforms; this just toggles which measurement passes run
/// and which manifest fields get populated.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPlatform {
    /// AMD SEV-SNP: build IGVM variants and populate `snp_variants[]`.
    Snp,
    /// Intel TDX: compute MRTD + RTMR[1] + RTMR[2] and populate `tdx`.
    Tdx,
    /// Both — default. Produces a manifest that attests on either fleet.
    Both,
}

impl BuildPlatform {
    pub fn needs_snp(self) -> bool {
        matches!(self, BuildPlatform::Snp | BuildPlatform::Both)
    }
    pub fn needs_tdx(self) -> bool {
        matches!(self, BuildPlatform::Tdx | BuildPlatform::Both)
    }
}

#[derive(clap::Args)]
pub struct KernelArgs {
    /// Force rebuild even if cache is current
    #[arg(short, long)]
    pub force: bool,

    /// Output directory.
    #[arg(short, long, default_value = "output/kernel")]
    pub output: PathBuf,

    /// Optional kernel config fragment, merged after required + hardening.
    /// Omitted: steep builds only its hardened required + hardening baseline.
    /// Lets a project enable extra kernel symbols without modifying steep.
    #[arg(long, value_name = "PATH")]
    pub kernel_config_fragment: Option<PathBuf>,

    /// Extra packages for the kernel-builder tools tree (build-time tools a
    /// fragment needs, e.g. `dwarves` for CONFIG_DEBUG_INFO_BTF). Repeatable
    /// and comma-separated.
    #[arg(
        long = "kernel-builder-package",
        value_name = "PKG",
        value_delimiter = ','
    )]
    pub kernel_builder_package: Vec<String>,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Output directory from steep build
    #[arg(default_value = "output/base")]
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

    /// Attach an ephemeral encrypted scratch disk of this size (e.g. "20G") as
    /// the writable overlay upper layer. Creates a fresh raw disk in the
    /// output directory and attaches it with serial=confai-scratch so the
    /// guest initrd encrypts it; contents do not survive a reboot.
    #[arg(long, value_name = "SIZE")]
    pub scratch: Option<String>,
}

#[derive(clap::Args)]
pub struct BuildArgs {
    /// Output directory for artifacts (IGVM, UKI, manifest, disk image)
    #[arg(default_value = "base")]
    pub name: PathBuf,

    /// Path to cloud-init user-data file to optionally include in the image
    #[arg(short, long)]
    pub cloud_init: Option<PathBuf>,

    /// Directory of extra files to bake into the image. Contents are copied
    /// into the image filesystem root, layered on top of the base image.
    #[arg(short = 'e', long, value_name = "DIR")]
    pub extra: Option<PathBuf>,

    /// Extra package to install in the base image. Repeatable and
    /// comma-separated. Passed through to mkosi as `--package`.
    #[arg(
        short = 'p',
        long = "package",
        value_name = "PKG",
        value_delimiter = ','
    )]
    pub package: Vec<String>,

    /// Extra package to install in the kernel-builder tools tree (where the
    /// custom kernel is compiled), not the final image. Repeatable and
    /// comma-separated. Use for build-time tools a fragment needs — e.g.
    /// `dwarves` (pahole) when the fragment enables CONFIG_DEBUG_INFO_BTF.
    /// Twin of `--package`, routed to the kernel-builder mkosi run.
    #[arg(
        long = "kernel-builder-package",
        value_name = "PKG",
        value_delimiter = ','
    )]
    pub kernel_builder_package: Vec<String>,

    /// Optional kernel config fragment, merged after required + hardening
    /// when building the custom kernel. Omitted: steep's hardened baseline.
    #[arg(long, value_name = "PATH")]
    pub kernel_config_fragment: Option<PathBuf>,

    /// Path to a post-install script to run during the build. Passed through
    /// to mkosi as --postinst-script, with --with-network=yes so the script
    /// can download resources from the network.
    #[arg(short = 's', long, value_name = "FILE")]
    pub script: Option<PathBuf>,

    /// Which confidential-VM platform(s) to measure for. `both` (default)
    /// emits SNP IGVM variants AND a TDX measurement block in the manifest.
    /// `snp` is IGVM-only (no TDX measurements). `tdx` skips IGVM
    /// generation entirely — same shape as the legacy `--skip-igvm`.
    #[arg(long, value_enum, default_value = "both")]
    pub platform: BuildPlatform,

    /// DEPRECATED: alias for `--platform tdx`. Use that instead.
    /// Skip IGVM generation (produce only disk + UKI, no SNP measurement).
    #[arg(long)]
    pub skip_igvm: bool,

    /// Path to OVMF firmware binary used for SNP launch. Must be steep's
    /// edk2 build that includes the IgvmHobArea region (region type 0x200),
    /// because IGVM construction injects the UKI/shim into that area. The
    /// upstream Ubuntu OVMF does NOT have this region and will fail
    /// IGVM build.
    #[arg(long, env = "STEEP_FIRMWARE", default_value = "output/OVMF.fd")]
    pub firmware: PathBuf,

    /// Path to OVMF firmware binary used for TDX measurement. Must be a
    /// build with TDVF code paths compiled in (TD HOB processing, TDCALL
    /// plumbing). Ubuntu's `ovmf` package binary at
    /// `/usr/share/ovmf/OVMF.fd` works. Steep's IGVM-aware firmware does
    /// NOT include TDVF and will hang silently when booted as a TDX guest.
    ///
    /// If --platform is `snp`, this firmware is ignored. If --platform is
    /// `tdx` or `both`, the TDX `mrtd` in the manifest is the hash of
    /// THIS firmware (not --firmware).
    #[arg(
        long = "tdx-firmware",
        env = "STEEP_TDX_FIRMWARE",
        default_value = "/usr/share/ovmf/OVMF.fd"
    )]
    pub tdx_firmware: PathBuf,

    /// RAM for VM (QEMU-style suffix, e.g. "4G")
    #[arg(long, default_value = "4G")]
    pub memory: String,

    /// SMP counts to generate IGVM variants for. Each value produces one
    /// `guest-smp{N}.igvm` file alongside a manifest entry under
    /// `variants[]`. Defaults to the standard powers-of-two set so a
    /// single `steep build` is enough to serve any common vCPU topology;
    /// `steep igvm` is then only needed for unusual SMP values or repair.
    #[arg(long, num_args = 1.., default_values_t = [2u32, 4, 8, 16])]
    pub smp: Vec<u32>,

    /// Enable an mkosi profile from `mkosi/base/mkosi.profiles/<NAME>/`.
    /// Repeatable. Profiles compose extra config (packages, systemd units,
    /// files) into the base image at build time. Each enabled profile may
    /// also trigger pre-build hooks (e.g. fetching binaries from GHCR).
    /// Currently supported: `attest` (bakes the attestation-api HTTP service)
    /// and `dev` (serial-console autologin + ttyS0 output for debugging).
    #[arg(long = "profile", value_name = "NAME")]
    pub profiles: Vec<String>,
}

#[derive(clap::Args)]
pub struct IgvmArgs {
    /// Build output directory (from steep build)
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

    /// OCI registry (e.g. ghcr.io/confidential-dot-ai/steep)
    #[arg(
        long,
        env = "STEEP_OCI_REGISTRY",
        default_value = "docker.io/confidentialai"
    )]
    pub registry: String,

    /// Image name
    #[arg(long)]
    pub name: Option<String>,

    /// Image tag
    #[arg(long, default_value = "latest")]
    pub tag: String,

    /// Build a CDI-compatible single-layer tar+gzip image (for KubeVirt CDI importer).
    ///
    /// When set, all files are packed into one `application/vnd.oci.image.layer.v1.tar+gzip`
    /// layer where `disk.raw` lives under `disk/` and other files sit at the tar root.
    #[arg(long, default_value_t = false)]
    pub cdi: bool,
}

#[derive(clap::Args)]
pub struct PullArgs {
    /// Image name to pull (e.g. "base")
    pub name: String,

    /// OCI registry
    #[arg(
        long,
        env = "STEEP_OCI_REGISTRY",
        default_value = "ghcr.io/confidential-dot-ai/steep"
    )]
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
