use std::path::PathBuf;
use steep::igvm::invoke::IgvmBuildArgs;

#[test]
fn test_igvm_build_args_to_command() {
    let args = IgvmBuildArgs {
        igvm_tools_bin: PathBuf::from("/path/to/igvm-tools"),
        firmware: PathBuf::from("/path/to/OVMF.fd"),
        kernel: PathBuf::from("/path/to/uki.efi"),
        smp: 4,
        manifest: Some(PathBuf::from("/path/to/manifest.json")),
        output: PathBuf::from("/path/to/guest.igvm"),
    };
    let cmd_args = args.to_args();
    assert_eq!(
        cmd_args,
        vec![
            "build",
            "--firmware", "/path/to/OVMF.fd",
            "--kernel", "/path/to/uki.efi",
            "--smp", "4",
            "--platform", "snp",
            "--manifest", "/path/to/manifest.json",
            "-o", "/path/to/guest.igvm",
        ]
    );
}

#[test]
fn test_igvm_build_args_without_manifest() {
    let args = IgvmBuildArgs {
        igvm_tools_bin: PathBuf::from("/path/to/igvm-tools"),
        firmware: PathBuf::from("/path/to/OVMF.fd"),
        kernel: PathBuf::from("/path/to/uki.efi"),
        smp: 1,
        manifest: None,
        output: PathBuf::from("/path/to/guest.igvm"),
    };
    let cmd_args = args.to_args();
    assert!(!cmd_args.contains(&"--manifest".to_string()));
}
