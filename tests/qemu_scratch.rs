use std::path::PathBuf;
use steep::qemu::{QemuArgs, QemuTier};

/// The scratch disk is found via an ext4 `LABEL=scratch` marker, so `cryptsetup`
/// sees an existing signature when it opens the device. Without `--batch-mode`
/// it blocks on an interactive "Are you sure?" confirmation that the stdin-less
/// initrd can never answer, hanging the boot. Guard against that regression.
#[test]
fn initrd_opens_scratch_non_interactively() {
    let init = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/mkosi/initrd/mkosi.extra/init"
    ))
    .unwrap();
    let open_line = init
        .lines()
        .find(|l| l.contains("cryptsetup open"))
        .expect("init must open the scratch device with cryptsetup");
    assert!(
        open_line.contains("--batch-mode") || open_line.contains("-q"),
        "cryptsetup open must be non-interactive (--batch-mode), else boot hangs \
         on the ext4-signature confirmation prompt. Got: {open_line}"
    );
}

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
