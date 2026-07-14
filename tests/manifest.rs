use confos::manifest::{
    BuildConfig, BuildManifest, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
    SnpVariant, TdxMeasurement, MANIFEST_VERSION,
};

fn sample_entry(path: &str) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        sha256: "abc123".to_string(),
    }
}

fn sample_measurement(digest: &str, vmsa_count: u32) -> Measurement {
    Measurement {
        snp_launch_digest: digest.to_string(),
        algorithm: "sha384".to_string(),
        page_count: 5598,
        vmsa_count,
    }
}

fn sample_variant(smp: u32, digest: &str) -> SnpVariant {
    SnpVariant {
        smp,
        igvm: FileEntry {
            path: format!("guest-smp{smp}.igvm"),
            sha256: format!("hash{smp}"),
        },
        measurement: sample_measurement(digest, smp),
    }
}

fn sample_tdx() -> TdxMeasurement {
    TdxMeasurement {
        mrtd: "11".repeat(48),
        rtmr1: "22".repeat(48),
        rtmr2: "33".repeat(48),
        firmware: Some(FileEntry {
            path: "OVMF.tdx.fd".to_string(),
            sha256: "ee".repeat(32),
        }),
    }
}

fn sample_manifest() -> BuildManifest {
    BuildManifest {
        version: MANIFEST_VERSION,
        build: BuildConfig {
            timestamp: "2026-03-13T12:00:00Z".to_string(),
            memory: "2G".to_string(),
            format: "raw".to_string(),
            platform: "snp".to_string(),
        },
        inputs: ManifestInputs {
            kernel: None,
            initrd: sample_entry("initrd.cpio.gz"),
            firmware: Some(sample_entry("OVMF.fd")),
            base_image: sample_entry("base.raw"),
        },
        outputs: ManifestOutputs {
            disk_image: sample_entry("disk.raw"),
            uki: sample_entry("uki.efi"),
        },
        snp_variants: vec![sample_variant(4, "aabbcc")],
        tdx: None,
    }
}

#[test]
fn manifest_serializes_to_json() {
    let manifest = sample_manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("\"version\": 3"));
    assert!(json.contains("\"snp_launch_digest\": \"aabbcc\""));
    assert!(json.contains("\"vmsa_count\": 4"));
    assert!(json.contains("\"firmware\""));
    assert!(json.contains("\"base_image\""));
    assert!(json.contains("\"snp_variants\""));
}

#[test]
fn manifest_roundtrip() {
    let manifest = sample_manifest();
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.version, manifest.version);
    assert_eq!(deserialized.snp_variants.len(), 1);
    assert_eq!(deserialized.snp_variants[0].smp, 4);
    assert_eq!(
        deserialized.snp_variants[0].measurement.snp_launch_digest,
        "aabbcc"
    );
    assert_eq!(deserialized.inputs.initrd.path, "initrd.cpio.gz");
    assert_eq!(deserialized.outputs.disk_image.path, "disk.raw");
    assert!(deserialized.tdx.is_none());
}

#[test]
fn manifest_roundtrip_multi_variant() {
    let mut manifest = sample_manifest();
    manifest.snp_variants = vec![
        sample_variant(2, "digest2"),
        sample_variant(4, "digest4"),
        sample_variant(8, "digest8"),
    ];
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.snp_variants.len(), 3);
    let digests: Vec<String> = deserialized
        .snp_variants
        .iter()
        .map(|v| v.measurement.snp_launch_digest.clone())
        .collect();
    assert_eq!(digests, vec!["digest2", "digest4", "digest8"]);
}

#[test]
fn manifest_optional_fields_omitted() {
    // With no SNP variants and no TDX block, both fields are skipped from
    // the serialized JSON (they default back on the read side). The
    // optional `firmware` input is also omitted when None.
    let mut manifest = sample_manifest();
    manifest.inputs.firmware = None;
    manifest.snp_variants.clear();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(!json.contains("firmware"));
    assert!(
        !json.contains("snp_variants"),
        "empty snp_variants should not appear: {json}"
    );
    assert!(!json.contains("\"tdx\""), "absent tdx should not appear");
}

#[test]
fn manifest_round_trips_with_tdx_block() {
    // A v3 manifest carrying both SNP variants and the TDX block must
    // round-trip cleanly. The TDX block stays a singleton.
    let mut manifest = sample_manifest();
    manifest.tdx = Some(sample_tdx());
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains("\"mrtd\""));
    assert!(json.contains("\"rtmr1\""));
    assert!(json.contains("\"rtmr2\""));

    let back: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tdx, Some(sample_tdx()));
    assert_eq!(back.snp_variants.len(), 1);
}

#[test]
fn manifest_tdx_only_serializes_without_snp_variants() {
    // A TDX-only build (no SNP variants) writes a manifest with the tdx
    // block present and the snp_variants field skipped.
    let mut manifest = sample_manifest();
    manifest.snp_variants.clear();
    manifest.tdx = Some(sample_tdx());
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(!json.contains("snp_variants"));
    assert!(json.contains("\"tdx\""));

    let back: BuildManifest = serde_json::from_str(&json).unwrap();
    assert!(back.snp_variants.is_empty());
    assert_eq!(back.tdx, Some(sample_tdx()));
}

#[test]
fn sha256_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.bin");
    fs_err::write(&path, b"hello world").unwrap();
    let hash = confos::manifest::sha256_file(&path).unwrap();
    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn read_manifest_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    let manifest = sample_manifest();
    confos::manifest::write_manifest(&manifest, &path).unwrap();
    let loaded = confos::manifest::read_manifest(&path).unwrap();
    assert_eq!(loaded.snp_variants[0].smp, 4);
    assert_eq!(loaded.build.memory, "2G");
    assert_eq!(loaded.build.format, "raw");
}

#[test]
fn manifest_rejects_unknown_fields_in_build() {
    let json = r#"{
        "version": 3,
        "build": {
            "timestamp": "2026-03-13T12:00:00Z",
            "memory": "2G",
            "format": "raw",
            "platform": "snp",
            "extra_bad_field": true
        },
        "inputs": {
            "initrd": {"path": "a", "sha256": "b"},
            "base_image": {"path": "c", "sha256": "d"}
        },
        "outputs": {
            "disk_image": {"path": "e", "sha256": "f"},
            "uki": {"path": "g", "sha256": "h"}
        },
        "snp_variants": []
    }"#;
    let result: Result<BuildManifest, _> = serde_json::from_str(json);
    assert!(
        result.is_err(),
        "should reject unknown fields in build config"
    );
}

#[test]
fn manifest_rejects_unknown_top_level_field() {
    let json = r#"{
        "version": 3,
        "build": {
            "timestamp": "2026-03-13T12:00:00Z",
            "memory": "2G",
            "format": "raw",
            "platform": "snp"
        },
        "inputs": {
            "initrd": {"path": "a", "sha256": "b"},
            "base_image": {"path": "c", "sha256": "d"}
        },
        "outputs": {
            "disk_image": {"path": "e", "sha256": "f"},
            "uki": {"path": "g", "sha256": "h"}
        },
        "snp_variants": [],
        "injected": "malicious"
    }"#;
    let result: Result<BuildManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "should reject unknown top-level fields");
}

#[test]
fn manifest_rejects_v2_legacy_fields() {
    // v2 carried `variants` (now renamed `snp_variants`). With
    // deny_unknown_fields a v2 manifest must fail to parse so a v2 file
    // can't be silently mis-read by a v3 consumer (`confos run` would
    // then see no SNP variants and refuse to launch on SNP hardware
    // rather than blindly picking up the wrong field).
    let json = r#"{
        "version": 2,
        "build": {
            "timestamp": "2026-03-13T12:00:00Z",
            "memory": "2G",
            "format": "raw",
            "platform": "snp"
        },
        "inputs": {
            "initrd": {"path": "a", "sha256": "b"},
            "base_image": {"path": "c", "sha256": "d"}
        },
        "outputs": {
            "disk_image": {"path": "e", "sha256": "f"},
            "uki": {"path": "g", "sha256": "h"}
        },
        "variants": []
    }"#;
    let result: Result<BuildManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "v2 manifest should not parse as v3");
}

#[test]
fn read_manifest_rejects_older_version() {
    // read_manifest peeks at the version field before deserialization and
    // refuses non-v3 manifests with a clear error.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2_manifest.json");
    let json = r#"{
        "version": 2,
        "build": {
            "timestamp": "2026-03-13T12:00:00Z",
            "memory": "2G",
            "format": "raw",
            "platform": "snp"
        },
        "inputs": {
            "initrd": {"path": "a", "sha256": "b"},
            "base_image": {"path": "c", "sha256": "d"}
        },
        "outputs": {
            "disk_image": {"path": "e", "sha256": "f"},
            "uki": {"path": "g", "sha256": "h"}
        },
        "variants": []
    }"#;
    fs_err::write(&path, json).unwrap();
    let err = confos::manifest::read_manifest(&path).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("version 2") && msg.contains("v3"),
        "expected v2-vs-v3 version error, got: {msg}"
    );
}

#[test]
fn sha256_file_nonexistent() {
    let result = confos::manifest::sha256_file(std::path::Path::new("/nonexistent/file.bin"));
    assert!(result.is_err());
}

#[test]
fn sha256_file_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.bin");
    fs_err::write(&path, b"").unwrap();
    let hash = confos::manifest::sha256_file(&path).unwrap();
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn write_manifest_creates_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_manifest.json");
    let manifest = sample_manifest();
    confos::manifest::write_manifest(&manifest, &path).unwrap();

    let content = fs_err::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["version"], 3);
    assert_eq!(value["snp_variants"][0]["smp"], 4);
}

#[test]
fn read_manifest_nonexistent_file() {
    let result =
        confos::manifest::read_manifest(std::path::Path::new("/nonexistent/manifest.json"));
    assert!(result.is_err());
}

#[test]
fn read_manifest_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    fs_err::write(&path, "not json").unwrap();
    let result = confos::manifest::read_manifest(&path);
    assert!(result.is_err());
}

#[test]
fn basename_of_strips_directories() {
    use std::path::Path;
    assert_eq!(
        confos::manifest::basename_of(Path::new("/abs/path/to/disk.raw")),
        "disk.raw"
    );
    assert_eq!(
        confos::manifest::basename_of(Path::new("relative/dir/OVMF.fd")),
        "OVMF.fd"
    );
    assert_eq!(
        confos::manifest::basename_of(Path::new("uki.efi")),
        "uki.efi"
    );
}
