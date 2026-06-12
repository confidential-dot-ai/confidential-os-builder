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
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    assert!(cmd.contains(&"-display".to_string()));
    assert!(cmd.contains(&"-serial".to_string()));
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
        scratch: None,
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
        scratch: None,
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
        scratch: None,
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
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(!cmd.contains(&"-netdev".to_string()));
    assert!(!joined.contains("virtio-net-pci"));
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
        scratch: None,
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
        scratch: None,
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
        scratch: None,
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
        scratch: None,
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

// --- validate_memory tests ---

use steep::qemu::validate_memory;

#[test]
fn test_validate_memory_valid_formats() {
    assert!(validate_memory("4G").is_ok());
    assert!(validate_memory("512M").is_ok());
    assert!(validate_memory("1024K").is_ok());
    assert!(validate_memory("1T").is_ok());
    assert!(validate_memory("2048").is_ok());
    assert!(validate_memory("4g").is_ok());
    assert!(validate_memory("512m").is_ok());
}

#[test]
fn test_validate_memory_empty() {
    assert!(validate_memory("").is_err());
}

#[test]
fn test_validate_memory_rejects_comma_injection() {
    assert!(validate_memory("4G,share=true").is_err());
}

#[test]
fn test_validate_memory_rejects_non_numeric() {
    assert!(validate_memory("abc").is_err());
    assert!(validate_memory("4GB").is_err());
    assert!(validate_memory("4 G").is_err());
    assert!(validate_memory("-4G").is_err());
    assert!(validate_memory("4G;echo").is_err());
}

// --- comma-in-path rejection tests ---

#[test]
fn test_qemu_args_rejects_comma_in_disk_path() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/my,disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}

#[test]
fn test_qemu_args_rejects_comma_in_igvm_path() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest,evil.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}

#[test]
fn test_qemu_args_rejects_comma_in_uki_path() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki,bad.efi")),
        firmware: Some(PathBuf::from("/usr/share/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}

#[test]
fn test_qemu_args_rejects_comma_in_firmware_path() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: Some(PathBuf::from("/usr/share/OV,MF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}

// --- unsupported disk format ---

#[test]
fn test_qemu_args_rejects_unsupported_disk_format() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "vmdk".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("unsupported disk format"));
}

#[test]
fn test_qemu_args_accepts_qcow2_format() {
    let args = QemuArgs {
        tier: QemuTier::SevSnp,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: Some(PathBuf::from("/output/guest.igvm")),
        uki: None,
        firmware: None,
        disk: PathBuf::from("/output/disk.qcow2"),
        disk_format: "qcow2".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("format=qcow2"));
}

#[test]
fn test_qemu_args_disk_is_readonly() {
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
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(
        joined.contains("file=/output/disk.raw,format=raw,if=virtio,readonly=on"),
        "disk drive should be marked readonly so the same image can back multiple VMs concurrently"
    );
}

// --- KVM tier missing firmware ---

#[test]
fn test_qemu_args_kvm_missing_firmware_errors() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    assert!(args.to_args().is_err());
}

#[test]
fn test_qemu_args_emulated_missing_firmware_errors() {
    let args = QemuArgs {
        tier: QemuTier::Emulated,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: None,
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    assert!(args.to_args().is_err());
}

#[test]
fn test_qemu_args_uses_virtio_console() {
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
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(
        joined.contains("virtio-serial-pci"),
        "missing virtio-serial-pci"
    );
    assert!(joined.contains("virtconsole"), "missing virtconsole device");
    assert!(
        joined.contains("chardev=hvc0"),
        "missing hvc0 chardev hookup"
    );
    // 8250 is gone — the SNP tier no longer uses -serial mon:stdio.
    assert!(!joined.contains("-serial mon:stdio"));
    // Monitor is multiplexed onto stdio so users can reach it with Ctrl-A C.
    assert!(joined.contains("mux=on"), "stdio chardev should be muxed");
    assert!(
        joined.contains("chardev=hvc0,mode=readline"),
        "monitor should be attached to the hvc0 mux"
    );
    assert!(
        !joined.contains("-monitor none"),
        "SNP tier should expose the monitor via the stdio mux"
    );
}

#[test]
fn test_qemu_args_kvm_uses_virtio_console() {
    let args = QemuArgs {
        tier: QemuTier::Kvm,
        qemu_bin: "qemu-system-x86_64".to_string(),
        igvm: None,
        uki: Some(PathBuf::from("/output/uki.efi")),
        firmware: Some(PathBuf::from("/usr/share/OVMF/OVMF.fd")),
        disk: PathBuf::from("/output/disk.raw"),
        disk_format: "raw".to_string(),
        smp: 1,
        memory: "2G".to_string(),
        port_forwards: vec![],
        scratch: None,
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(joined.contains("virtio-serial-pci"));
    assert!(joined.contains("chardev=hvc0"));
    assert!(joined.contains("mux=on"), "stdio chardev should be muxed");
    assert!(
        joined.contains("chardev=hvc0,mode=readline"),
        "monitor should be attached to the hvc0 mux"
    );
}

// --- parse_size_to_bytes tests ---

use steep::qemu::parse_size_to_bytes;

#[test]
fn test_parse_size_suffixes() {
    assert_eq!(parse_size_to_bytes("1024").unwrap(), 1024);
    assert_eq!(parse_size_to_bytes("1K").unwrap(), 1024);
    assert_eq!(parse_size_to_bytes("2M").unwrap(), 2 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("20G").unwrap(), 20u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
    assert_eq!(parse_size_to_bytes("4g").unwrap(), 4u64 * 1024 * 1024 * 1024);
}

#[test]
fn test_parse_size_rejects_garbage() {
    assert!(parse_size_to_bytes("").is_err());
    assert!(parse_size_to_bytes("20GB").is_err());
    assert!(parse_size_to_bytes("abc").is_err());
    assert!(parse_size_to_bytes("-5G").is_err());
    assert!(parse_size_to_bytes("5 G").is_err());
}
