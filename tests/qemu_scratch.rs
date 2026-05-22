use std::path::PathBuf;
use steep::qemu::{QemuArgs, QemuTier};

#[test]
fn test_qemu_args_scratch_adds_writable_drive() {
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
        scratch: Some(PathBuf::from("/output/scratch.raw")),
    };
    let cmd = args.to_args().unwrap();
    let joined = cmd.join(" ");
    assert!(
        joined.contains("file=/output/scratch.raw,format=raw,if=virtio"),
        "scratch drive missing: {joined}"
    );
    assert!(
        !joined.contains("file=/output/scratch.raw,format=raw,if=virtio,readonly=on"),
        "scratch drive must be writable"
    );
}

#[test]
fn test_qemu_args_no_scratch_adds_no_second_drive() {
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
    let drive_count = cmd.iter().filter(|s| *s == "-drive").count();
    assert_eq!(drive_count, 1, "expected only the root drive");
}

#[test]
fn test_qemu_args_rejects_comma_in_scratch_path() {
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
        scratch: Some(PathBuf::from("/output/scr,atch.raw")),
    };
    let err = args.to_args().unwrap_err();
    assert!(err.to_string().contains("comma"));
}
