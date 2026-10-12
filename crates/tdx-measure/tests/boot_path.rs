//! RTMR[1] for the two ESP layouts confos builds: systemd-boot at
//! `\EFI\BOOT\BOOTX64.EFI`, which loads the UKI (the default), or the UKI
//! itself at that path (`Bootloader=uki`, the `gcp` profile).

use std::io::{Cursor, Write};

use sha2::{Digest, Sha384};
use tdx_measure::pe::authenticode_sha384;
use tdx_measure::rtmr::compute_rtmr1_uki;

/// A PE32+ image holding `sections`, enough for the PE parser and Authenticode.
fn pe(sections: &[(&str, &[u8])]) -> Vec<u8> {
    let mut image = vec![0u8; 512];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    image[coff + 2..coff + 4].copy_from_slice(&(sections.len() as u16).to_le_bytes());
    let optional_header_size = 112 + 16 * 8;
    image[coff + 16..coff + 18].copy_from_slice(&(optional_header_size as u16).to_le_bytes());
    let opt = coff + 20;
    image[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    image[opt + 60..opt + 64].copy_from_slice(&512u32.to_le_bytes()); // SizeOfHeaders
    for (i, (name, data)) in sections.iter().enumerate() {
        let header = opt + optional_header_size + i * 40;
        let raw = image.len();
        let raw_size = data.len().div_ceil(512) * 512;
        image[header..header + name.len()].copy_from_slice(name.as_bytes());
        image[header + 8..header + 12].copy_from_slice(&(data.len() as u32).to_le_bytes());
        image[header + 16..header + 20].copy_from_slice(&(raw_size as u32).to_le_bytes());
        image[header + 20..header + 24].copy_from_slice(&(raw as u32).to_le_bytes());
        image.extend_from_slice(data);
        image.resize(raw + raw_size, 0);
    }
    image
}

fn uki() -> Vec<u8> {
    pe(&[(".linux", b"kernel"), (".cmdline", b"console=ttyS0")])
}

fn systemd_boot() -> Vec<u8> {
    pe(&[(".text", b"systemd-boot")])
}

const ESP_FIRST_LBA: usize = 34;
const ESP_SECTORS: usize = 2048;

/// A GPT disk whose only partition is a FAT ESP with `bootx64` at
/// `EFI/BOOT/BOOTX64.EFI`.
fn disk(bootx64: &[u8]) -> Vec<u8> {
    let mut disk = vec![0u8; (ESP_FIRST_LBA + ESP_SECTORS) * 512];
    let header = &mut disk[512..1024];
    header[..8].copy_from_slice(b"EFI PART");
    header[12..16].copy_from_slice(&92u32.to_le_bytes());
    header[72..80].copy_from_slice(&2u64.to_le_bytes()); // partition entries at LBA 2
    header[80..84].copy_from_slice(&4u32.to_le_bytes());
    header[84..88].copy_from_slice(&128u32.to_le_bytes());
    let entry = &mut disk[1024..1152];
    entry[..16].copy_from_slice(&[
        0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9,
        0x3b,
    ]);
    entry[16..32].copy_from_slice(&[0x11; 16]);
    entry[32..40].copy_from_slice(&(ESP_FIRST_LBA as u64).to_le_bytes());
    entry[40..48].copy_from_slice(&((ESP_FIRST_LBA + ESP_SECTORS - 1) as u64).to_le_bytes());

    let esp = &mut disk[ESP_FIRST_LBA * 512..];
    fatfs::format_volume(Cursor::new(&mut *esp), fatfs::FormatVolumeOptions::new()).unwrap();
    let fs = fatfs::FileSystem::new(Cursor::new(esp), fatfs::FsOptions::new()).unwrap();
    let efi = fs.root_dir().create_dir("EFI").unwrap();
    let mut file = efi
        .create_dir("BOOT")
        .unwrap()
        .create_file("BOOTX64.EFI")
        .unwrap();
    file.write_all(bootx64).unwrap();
    drop((file, efi));
    fs.unmount().unwrap();
    disk
}

/// EV_EFI_GPT_EVENT: the GPT header, the number of used entries, the entries.
fn gpt_event(disk: &[u8]) -> Vec<u8> {
    let mut data = disk[512..512 + 92].to_vec();
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&disk[1024..1152]);
    Sha384::digest(&data).to_vec()
}

fn replay(events: &[Vec<u8>]) -> Vec<u8> {
    events.iter().fold(vec![0; 48], |mr, event| {
        Sha384::digest([mr.as_slice(), event].concat()).to_vec()
    })
}

/// The RTMR[1] log with `loads` as the image-load events.
fn rtmr1(disk: &[u8], loads: &[&[u8]]) -> Vec<u8> {
    let mut events = vec![
        Sha384::digest(b"Calling EFI Application from Boot Option").to_vec(),
        Sha384::digest([0u8; 4]).to_vec(),
        gpt_event(disk),
    ];
    events.extend(loads.iter().map(|pe| authenticode_sha384(pe).unwrap()));
    events.push(Sha384::digest(b"Exit Boot Services Invocation").to_vec());
    events.push(Sha384::digest(b"Exit Boot Services Returned with Success").to_vec());
    replay(&events)
}

#[test]
fn uki_as_bootx64_logs_one_image_load() {
    let uki = uki();
    let disk = disk(&uki);
    let rtmr1_value = compute_rtmr1_uki(&uki, &[], Some(&disk)).unwrap();
    assert_eq!(rtmr1_value, rtmr1(&disk, &[&uki]));
}

#[test]
fn systemd_boot_as_bootx64_logs_it_before_the_uki() {
    let (uki, bootloader) = (uki(), systemd_boot());
    let disk = disk(&bootloader);
    let rtmr1_value = compute_rtmr1_uki(&uki, &[], Some(&disk)).unwrap();
    assert_eq!(rtmr1_value, rtmr1(&disk, &[&bootloader, &uki]));
}
