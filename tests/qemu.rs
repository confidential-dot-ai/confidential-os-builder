use std::path::PathBuf;
use steep::qemu::QemuArgs;

#[test]
fn test_qemu_args_basic() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 2,
        memory: "2G".to_string(),
    };
    let cmd = args.to_args();
    assert!(cmd.contains(&"-nographic".to_string()));
    assert!(cmd.contains(&"-smp".to_string()));
    assert!(cmd.contains(&"2".to_string()));
    assert!(cmd.contains(&"-m".to_string()));
    assert!(cmd.contains(&"2G".to_string()));
}

#[test]
fn test_qemu_args_contains_sev_snp() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 1,
        memory: "4G".to_string(),
    };
    let cmd = args.to_args();
    let joined = cmd.join(" ");
    assert!(joined.contains("confidential-guest-support=sev0"));
    assert!(joined.contains("sev-snp-guest"));
    assert!(joined.contains("igvm-cfg"));
}

#[test]
fn test_qemu_args_disk_format() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.vhd"),
        disk_format: "vpc".to_string(),
        smp: 1,
        memory: "2G".to_string(),
    };
    let cmd = args.to_args();
    let joined = cmd.join(" ");
    assert!(joined.contains("format=vpc"));
}
