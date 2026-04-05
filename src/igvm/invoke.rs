use std::path::PathBuf;

use crate::tools;

/// Arguments for `igvm-tools build`.
pub struct IgvmBuildArgs {
    pub igvm_tools_bin: PathBuf,
    pub firmware: PathBuf,
    pub kernel: PathBuf,
    pub smp: u32,
    pub manifest: Option<PathBuf>,
    pub output: PathBuf,
}

impl IgvmBuildArgs {
    /// Convert to command-line argument list for igvm-tools.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "build".to_string(),
            "--firmware".to_string(),
            self.firmware.display().to_string(),
            "--kernel".to_string(),
            self.kernel.display().to_string(),
            "--smp".to_string(),
            self.smp.to_string(),
            "--platform".to_string(),
            "snp".to_string(),
        ];
        if let Some(ref manifest) = self.manifest {
            args.push("--manifest".to_string());
            args.push(manifest.display().to_string());
        }
        args.push("-o".to_string());
        args.push(self.output.display().to_string());
        args
    }
}

/// Invoke `igvm-tools build` with the given arguments.
pub fn build(args: &IgvmBuildArgs) -> anyhow::Result<()> {
    let bin = args.igvm_tools_bin.to_string_lossy();
    let cmd_args = args.to_args();
    tracing::info!(output = %args.output.display(), smp = args.smp, "invoking igvm-tools build");
    tools::run_command_streaming(&bin, &cmd_args)?;
    Ok(())
}
