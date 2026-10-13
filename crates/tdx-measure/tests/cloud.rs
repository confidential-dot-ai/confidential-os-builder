//! The cloud predictor against what GCE and Azure actually measured when they
//! booted confos images. The boot digests were computed by
//! `BootDigests::from_image` from those images (`real_images_*` re-derives
//! them when the images are available); the event log, quote and TPM quote
//! are the captured evidence.

use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use tdx_measure::alg::HashAlg;
use tdx_measure::ccel;
use tdx_measure::cloud::{AzureVtpm, BootDigests, BootPath, GcpTdx};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cloud");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(FIXTURES).join(name)).unwrap()
}

fn json(name: &str) -> Value {
    serde_json::from_slice(&fixture(name)).unwrap()
}

fn unhex(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn digests(name: &str, alg: HashAlg) -> BootDigests {
    let f = json(name);
    let d = &f[match alg {
        HashAlg::Sha256 => "sha256",
        HashAlg::Sha384 => "sha384",
    }];
    let list = |key: &str| d[key].as_array().unwrap().iter().map(unhex).collect();
    BootDigests {
        alg,
        boot_path: match f["boot_path"].as_str().unwrap() {
            "systemd-boot" => BootPath::SystemdBoot,
            "direct-uki" => BootPath::DirectUki,
            other => panic!("boot_path {other}"),
        },
        gpt: unhex(&d["gpt"]),
        bootloader: (!d["bootloader"].is_null()).then(|| unhex(&d["bootloader"])),
        uki: unhex(&d["uki"]),
        uki_sections: list("uki_sections"),
        kernel_stub: list("kernel_stub"),
    }
}

/// RTMR[1] and RTMR[2] from a TDX v4 quote body (header 48 bytes; RTMR[0]
/// at body offset 328).
fn quote_rtmrs(quote: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (quote[424..472].to_vec(), quote[472..520].to_vec())
}

#[test]
fn gcp_prediction_equals_the_quote_from_a_direct_uki_boot() {
    let boot = digests("gcp-initfree0-boot-digests.json", HashAlg::Sha384);
    assert_eq!(boot.boot_path, BootPath::DirectUki);
    let predicted = GcpTdx::from_digests(&boot);
    let (rtmr1, rtmr2) = quote_rtmrs(&fixture("gcp-initfree0-boot1b-quote.bin"));
    assert_eq!(hex::encode(&predicted.rtmr1), hex::encode(rtmr1));
    assert_eq!(hex::encode(&predicted.rtmr2), hex::encode(rtmr2));
}

#[test]
fn gcp_event_log_holds_exactly_the_predicted_events() {
    let boot = digests("gcp-initfree0-boot-digests.json", HashAlg::Sha384);
    let events = ccel::parse_ccel(&fixture("gcp-initfree0-boot1b-ccel.bin")).unwrap();
    let logged = |mr_index| -> Vec<String> {
        events
            .iter()
            .filter(|e| e.mr_index == mr_index)
            .map(|e| hex::encode(&e.sha384_digest))
            .collect()
    };
    let hexes = |v: &[Vec<u8>]| -> Vec<String> { v.iter().map(hex::encode).collect() };
    // RTMR[1]: one image load (the UKI), no systemd-boot.
    assert_eq!(logged(2), hexes(&boot.tdx_rtmr1_events()));
    // RTMR[2]: systemd-stub's 12 section events; the kernel stub's command
    // line and initrd events are not in the CC log on GCE.
    assert_eq!(boot.uki_sections.len(), 12);
    assert_eq!(boot.kernel_stub.len(), 2);
    assert_eq!(logged(3), hexes(&boot.uki_sections));
    // The log is the one the quote covers.
    let replayed = ccel::replay_rtmrs(&events);
    let (rtmr1, rtmr2) = quote_rtmrs(&fixture("gcp-initfree0-boot1b-quote.bin"));
    assert_eq!((replayed[1].clone(), replayed[2].clone()), (rtmr1, rtmr2));
}

#[test]
fn gcp_systemd_boot_path_adds_one_image_load() {
    let mut boot = digests("gcp-initfree0-boot-digests.json", HashAlg::Sha384);
    let direct = boot.tdx_rtmr1_events();
    let bootloader = HashAlg::Sha384.hash(b"systemd-boot");
    boot.boot_path = BootPath::SystemdBoot;
    boot.bootloader = Some(bootloader.clone());
    let mut expected = direct.clone();
    expected.insert(3, bootloader);
    assert_eq!(boot.tdx_rtmr1_events(), expected);
}

#[test]
fn azure_prediction_equals_the_signed_vtpm_pcrs() {
    let boot = digests("azure-v012-boot-digests.json", HashAlg::Sha256);
    assert_eq!(boot.boot_path, BootPath::SystemdBoot);
    let predicted = AzureVtpm::from_digests(&boot);
    let quote = json("azure-v012-boot-tpm-quote.json");
    let pcrs: Vec<Vec<u8>> = quote["pcrs"]
        .as_array()
        .unwrap()
        .iter()
        .map(unhex)
        .collect();
    // The PCR values are the ones the quote signs: TPMS_ATTEST ends with the
    // PCR selection and the SHA-256 digest of the selected PCRs.
    let message = unhex(&quote["message"]);
    let digest = &message[message.len() - 32..];
    let select = &message[message.len() - 34 - 3..message.len() - 34];
    let selected: Vec<u8> = (0..24)
        .filter(|i| select[i / 8] & (1 << (i % 8)) != 0)
        .flat_map(|i| pcrs[i].clone())
        .collect();
    assert_eq!(Sha256::digest(&selected).as_slice(), digest);
    assert_eq!(select, [0xff, 0xff, 0xff]);
    assert_eq!(predicted.pcr4, pcrs[4]);
    assert_eq!(predicted.pcr9, pcrs[9]);
    assert_eq!(predicted.pcr11, pcrs[11]);
}

/// Recompute the fixture digests from the images they came from:
/// `CONFOS_CLOUD_IMAGES=<dir>` with `azure-vtpm-fix2/` and `gcp-initfree0/`,
/// each holding `uki.efi` and `disk.raw` (2 GB each; kept on the build host).
#[test]
#[ignore]
fn real_images_reproduce_the_fixture_digests() {
    let root = std::path::PathBuf::from(std::env::var("CONFOS_CLOUD_IMAGES").unwrap());
    for (dir, fixture_name, alg) in [
        (
            "azure-vtpm-fix2",
            "azure-v012-boot-digests.json",
            HashAlg::Sha256,
        ),
        (
            "gcp-initfree0",
            "gcp-initfree0-boot-digests.json",
            HashAlg::Sha384,
        ),
    ] {
        let uki = std::fs::read(root.join(dir).join("uki.efi")).unwrap();
        let disk = std::fs::read(root.join(dir).join("disk.raw")).unwrap();
        let computed = BootDigests::from_image(&uki, &disk, alg).unwrap();
        assert_eq!(computed, digests(fixture_name, alg), "{dir}");
    }
}
