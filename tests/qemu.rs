use std::path::PathBuf;
use steep::qemu::{select_tier, QemuArgs, QemuTier};

#[test]
fn test_qemu_args_basic() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 2,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    assert!(cmd.contains(&"-nographic".to_string()));
    assert!(cmd.contains(&"-smp".to_string()));
    assert!(cmd.contains(&"2".to_string()));
    assert!(cmd.contains(&"-m".to_string()));
    assert!(cmd.contains(&"2G".to_string()));
}

#[test]
fn test_qemu_args_contains_sev_snp() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "4G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("confidential-guest-support=sev0"));
    assert!(joined.contains("sev-snp-guest"));
    assert!(joined.contains("igvm-cfg"));
}

#[test]
fn test_qemu_args_snp_missing_igvm_errors() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    assert!(args.to_args().is_err());
}

#[test]
fn test_qemu_args_kvm_missing_uki_errors() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: None,
        firmware: Some(PathBuf::from("/usr/share/OVMF/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    assert!(args.to_args().is_err());
}

#[test]
fn test_qemu_args_no_port_forwards_has_no_netdev() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    assert!(!cmd.contains(&"-netdev".to_string()));
    assert!(!cmd.contains(&"-device".to_string()));
}

#[test]
fn test_qemu_args_single_port_forward() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![(8080, 80)],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("hostfwd=tcp::8080-:80"));
    assert!(joined.contains("virtio-net-pci,netdev=net0"));
}

#[test]
fn test_qemu_args_multiple_port_forwards() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![(8080, 80), (8443, 443)],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("hostfwd=tcp::8080-:80"));
    assert!(joined.contains("hostfwd=tcp::8443-:443"));
    let netdev_count = cmd.iter().filter(|s| *s == "-netdev").count();
    assert_eq!(netdev_count, 1);
}

#[test]
fn test_qemu_args_kvm_tier() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: Some(PathBuf::from("/usr/share/OVMF/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 2,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("-enable-kvm"));
    assert!(joined.contains("-kernel /output/uki.efi"));
    assert!(joined.contains("if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF.fd"));
    assert!(!joined.contains("sev-snp-guest"));
}

#[test]
fn test_qemu_args_emulated_tier() {
    let args = QemuArgs {
        tier: QemuTier::Emulated,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: Some(PathBuf::from("/usr/share/OVMF/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(!joined.contains("-enable-kvm"));
    assert!(joined.contains("-kernel /output/uki.efi"));
    assert!(!joined.contains("sev-snp-guest"));
}

// --- select_tier tests ---

#[test]
fn test_select_tier_sevsnp() {
    let output = "List of user creatable objects:\nsev-snp-guest\nigvm-cfg\nmemory-backend-file\n";
    assert_eq!(select_tier(output, true), QemuTier::SevSnp);
}

#[test]
fn test_select_tier_kvm() {
    let output = "List of user creatable objects:\nmemory-backend-file\n";
    assert_eq!(select_tier(output, true), QemuTier::Kvm);
}

#[test]
fn test_select_tier_emulated() {
    let output = "List of user creatable objects:\nmemory-backend-file\n";
    assert_eq!(select_tier(output, false), QemuTier::Emulated);
}

#[test]
fn test_select_tier_snp_objects_no_kvm() {
    let output = "List of user creatable objects:\nsev-snp-guest\nigvm-cfg\n";
    assert_eq!(select_tier(output, false), QemuTier::Emulated);
}
