use std::path::PathBuf;
use steep::uki::build::UkifyBuildArgs;

#[test]
fn test_ukify_args_basic() {
    let args = UkifyBuildArgs {
        kernel: PathBuf::from("/path/to/vmlinuz"),
        initrds: vec![PathBuf::from("/path/to/initrd.img")],
        output: PathBuf::from("/path/to/uki.efi"),
    };
    let cmd_args = args.to_args();
    assert_eq!(
        cmd_args,
        vec![
            "build",
            "--linux", "/path/to/vmlinuz",
            "--initrd", "/path/to/initrd.img",
            "--output", "/path/to/uki.efi",
        ]
    );
}

#[test]
fn test_ukify_args_multiple_initrds() {
    let args = UkifyBuildArgs {
        kernel: PathBuf::from("/path/to/vmlinuz"),
        initrds: vec![
            PathBuf::from("/path/to/initrd.img"),
            PathBuf::from("/path/to/verity-initrd.img"),
        ],
        output: PathBuf::from("/path/to/uki.efi"),
    };
    let cmd_args = args.to_args();
    let initrd_count = cmd_args.iter().filter(|a| *a == "--initrd").count();
    assert_eq!(initrd_count, 2);
}
