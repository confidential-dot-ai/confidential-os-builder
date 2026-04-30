//! Compile the kernel inside systemd-nspawn, with reproducibility env pinned.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// Compile the kernel; copy the resulting bzImage to `out_vmlinuz`.
/// Tees nspawn stdout/stderr to `log_path`.
pub fn run(
    tools_tree: &Path,
    kernel_dir: &Path,
    out_vmlinuz: &Path,
    log_path: &Path,
) -> Result<()> {
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // Truncate the build log up front so it always reflects this run.
    fs_err::write(log_path, b"")?;

    let kernel_dir_abs = kernel_dir.canonicalize()?;
    let log_abs = log_path.canonicalize()?;

    let env = [
        ("SOURCE_DATE_EPOCH", "0"),
        ("KBUILD_BUILD_TIMESTAMP", "@0"),
        ("KBUILD_BUILD_USER", "steep"),
        ("KBUILD_BUILD_HOST", "steep"),
        ("KCONFIG_NOTIMESTAMP", "1"),
    ];

    let script = format!(
        "set -eux\n\
         cd /build\n\
         make -j{parallelism} bzImage 2>&1 | tee -a /build.log\n\
         test -f arch/x86/boot/bzImage\n",
        parallelism = parallelism,
    );

    let nspawn_bin = crate::tools::require("systemd-nspawn")
        .map_err(|_| anyhow!("systemd-nspawn required; install systemd-container"))?;

    let mut args: Vec<std::ffi::OsString> = vec![
        "--quiet".into(),
        "--register=no".into(),
        "--keep-unit".into(),
        "--ephemeral".into(),
        "--directory".into(),
        tools_tree.into(),
        "--bind".into(),
        format!("{}:/build", kernel_dir_abs.display()).into(),
        "--bind".into(),
        format!("{}:/build.log", log_abs.display()).into(),
    ];
    for (k, v) in &env {
        args.push("--setenv".into());
        args.push(format!("{}={}", k, v).into());
    }
    args.push("/bin/bash".into());
    args.push("-c".into());
    args.push(script.into());

    let mut full_args: Vec<std::ffi::OsString> = vec![nspawn_bin.as_os_str().into()];
    full_args.extend(args);

    crate::tools::run_command_streaming("sudo", &full_args[..]).map_err(|e| {
        anyhow!(
            "kernel compile failed: {}. Full log: {}",
            e,
            log_path.display()
        )
    })?;

    let bz = kernel_dir_abs.join("arch/x86/boot/bzImage");
    if !bz.exists() {
        return Err(anyhow!(
            "kernel build claimed success but bzImage missing at {}",
            bz.display()
        ));
    }
    fs_err::copy(&bz, out_vmlinuz)
        .with_context(|| format!("copying bzImage to {}", out_vmlinuz.display()))?;
    Ok(())
}
