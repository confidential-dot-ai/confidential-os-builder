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
        port_forwards: vec![],
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
        port_forwards: vec![],
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
        port_forwards: vec![],
    };
    let cmd = args.to_args();
    let joined = cmd.join(" ");
    assert!(joined.contains("format=vpc"));
}

#[test]
fn test_qemu_args_no_port_forwards_has_no_netdev() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args();
    assert!(!cmd.contains(&"-netdev".to_string()));
    assert!(!cmd.contains(&"-device".to_string()));
}

#[test]
fn test_qemu_args_single_port_forward() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![(8080, 80)],
    };
    let cmd = args.to_args();
    let joined = cmd.join(" ");
    assert!(joined.contains("hostfwd=tcp::8080-:80"));
    assert!(joined.contains("virtio-net-pci,netdev=net0"));
}

#[test]
fn test_qemu_args_multiple_port_forwards() {
    let args = QemuArgs {
        igvm: PathBuf::from("/output/guest.igvm"),
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![(8080, 80), (8443, 443)],
    };
    let cmd = args.to_args();
    let joined = cmd.join(" ");
    assert!(joined.contains("hostfwd=tcp::8080-:80"));
    assert!(joined.contains("hostfwd=tcp::8443-:443"));
    let netdev_count = cmd.iter().filter(|s| *s == "-netdev").count();
    assert_eq!(netdev_count, 1);
}
