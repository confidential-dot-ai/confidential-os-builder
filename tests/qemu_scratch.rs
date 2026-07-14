use confos::qemu::{QemuArgs, QemuTier};
use std::path::PathBuf;

/// The PVC may carry stale filesystem signatures from a previous boot's
/// ciphertext that happen to overlap with ext4 magic bytes. cryptsetup detects
/// those signatures and, without `--batch-mode`, blocks on an interactive
/// "Are you sure?" confirmation the stdin-less initrd can never answer, hanging
/// the boot. Guard against that regression.
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
         on the stale-signature confirmation prompt. Got: {open_line}"
    );
}

/// The initrd gates the encrypted-scratch path on the virtio device serial
/// (`/sys/block/<dev>/serial == "confai-scratch"`). Guard against accidental
/// regression to LABEL-based or always-encrypt gating.
#[test]
fn initrd_gates_scratch_on_serial() {
    let init = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/mkosi/initrd/mkosi.extra/init"
    ))
    .unwrap();
    assert!(
        init.contains("/sys/block/") && init.contains("/serial"),
        "init must read /sys/block/<dev>/serial to gate the scratch path"
    );
    assert!(
        init.contains("confai-scratch"),
        "init must compare against the confai-scratch serial value"
    );
    assert!(
        !init.contains("blkid"),
        "init must not use blkid LABEL detection (replaced by serial gate)"
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
    // QEMU >= 3.0 removed the `serial=` sugar on -drive, so the serial must be
    // set on the virtio-blk device instead.
    assert!(
        joined.contains("-drive file=/output/scratch.raw,format=raw,if=none,id=scratch0"),
        "scratch drive missing or malformed: {joined}"
    );
    assert!(
        joined.contains("-device virtio-blk-pci,drive=scratch0,serial=confai-scratch"),
        "scratch virtio-blk device missing serial=confai-scratch: {joined}"
    );
    // Both disks must be explicit -device (not board-created if=virtio), and
    // root must come first: explicit devices get PCI slots in command-line
    // order, while board-created if=virtio devices are realized AFTER all
    // -device args — mixing the two put scratch at a lower slot than root,
    // so the guest saw scratch as vda and the initrd's /dev/vda2 mount hung.
    assert!(
        joined.contains("-drive file=/output/disk.raw,format=raw,if=none,id=root0,readonly=on"),
        "root drive must be explicit if=none: {joined}"
    );
    let root_dev = cmd
        .iter()
        .position(|s| s.starts_with("virtio-blk-pci,drive=root0"))
        .expect("root virtio-blk device must be present");
    let scratch_dev = cmd
        .iter()
        .position(|s| s.starts_with("virtio-blk-pci,drive=scratch0"))
        .expect("scratch virtio-blk device must be present");
    assert!(
        root_dev < scratch_dev,
        "root device must precede scratch device so root enumerates as vda: {joined}"
    );
    let scratch_drive = cmd
        .iter()
        .find(|s| s.contains("scratch.raw"))
        .expect("scratch drive must be present in args");
    assert!(
        !scratch_drive.contains("readonly=on"),
        "scratch drive must be writable: {scratch_drive}"
    );
    assert!(
        !scratch_drive.contains("serial="),
        "serial= on -drive was removed in QEMU 3.0; it must go on the -device: {scratch_drive}"
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
