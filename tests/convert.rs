use steep::convert;
use steep::ImageFormat;

#[test]
fn test_qemu_img_format_qcow2() {
    assert_eq!(convert::qemu_img_format(&ImageFormat::Qcow2), "qcow2");
}

#[test]
fn test_qemu_img_format_vhd() {
    assert_eq!(convert::qemu_img_format(&ImageFormat::Vhd), "vpc");
}

#[test]
fn test_qemu_img_format_raw() {
    assert_eq!(convert::qemu_img_format(&ImageFormat::Raw), "raw");
}

#[test]
fn test_convert_args() {
    let args = convert::convert_args(
        std::path::Path::new("/tmp/disk.raw"),
        std::path::Path::new("/tmp/disk.qcow2"),
        &ImageFormat::Qcow2,
    );
    assert_eq!(args, vec!["convert", "-f", "raw", "-O", "qcow2", "/tmp/disk.raw", "/tmp/disk.qcow2"]);
}

#[test]
fn test_convert_args_vhd_uses_vpc() {
    let args = convert::convert_args(
        std::path::Path::new("/tmp/disk.raw"),
        std::path::Path::new("/tmp/disk.vhd"),
        &ImageFormat::Vhd,
    );
    assert_eq!(args, vec!["convert", "-f", "raw", "-O", "vpc", "/tmp/disk.raw", "/tmp/disk.vhd"]);
}
