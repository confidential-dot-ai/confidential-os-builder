use std::path::PathBuf;

use crate::tools;

/// Arguments for `ukify build`.
pub struct UkifyBuildArgs {
    pub kernel: PathBuf,
    pub initrds: Vec<PathBuf>,
    pub cmdline: Option<String>,
    pub output: PathBuf,
}

impl UkifyBuildArgs {
    /// Convert to command-line argument list for ukify.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = vec![
            "build".to_string(),
            "--linux".to_string(),
            self.kernel.display().to_string(),
        ];
        for initrd in &self.initrds {
            args.push("--initrd".to_string());
            args.push(initrd.display().to_string());
        }
        if let Some(cmdline) = &self.cmdline {
            args.push("--cmdline".to_string());
            args.push(cmdline.clone());
        }
        args.push("--output".to_string());
        args.push(self.output.display().to_string());
        args
    }
}

/// Invoke `ukify build` to produce a UKI EFI binary.
/// Data flow: (kernel + initrd(s)) → ukify → UKI.efi
pub fn build(args: &UkifyBuildArgs) -> Result<(), tools::ToolError> {
    tools::require("ukify")?;
    let cmd_args = args.to_args();
    tracing::info!(output = %args.output.display(), "building UKI via ukify");
    tools::run_command_streaming("ukify", &cmd_args)
}
