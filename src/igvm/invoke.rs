use igvm_tools::{BootMode, BuildConfig, BuildResult, Platform};

/// Build an IGVM image from firmware and kernel (UKI) bytes.
///
/// Returns the serialized IGVM binary and SNP measurement result.
pub fn build_snp(firmware: &[u8], kernel: &[u8], smp: u32) -> anyhow::Result<BuildResult> {
    tracing::info!(smp, "building IGVM via igvm_tools library");

    let config = BuildConfig {
        firmware,
        kernel: Some(kernel),
        vars: None,
        shim: None,
        pk: None,
        kek: None,
        db: None,
        dbx: None,
        platform: Platform::Snp,
        boot_mode: BootMode::Real16,
        smp,
        verbose: false,
    };

    igvm_tools::build(&config).map_err(|e| anyhow::anyhow!("igvm build failed: {e}"))
}
