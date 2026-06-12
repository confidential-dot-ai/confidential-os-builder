use steep::manifest::{
    BuildConfig, BuildManifest, FileEntry, IgvmVariant, ManifestInputs, ManifestOutputs,
    Measurement, MANIFEST_VERSION,
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

fn sample_variant(smp: u32, digest: &str) -> IgvmVariant {
    IgvmVariant {
        smp,
        igvm: FileEntry {
            path: format!("guest-smp{smp}.igvm"),
            sha256: format!("hash{smp}"),
        },
        measurement: sample_measurement(digest, smp),
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
        variants: vec![sample_variant(4, "aabbcc")],
    }
}

#[test]
fn manifest_serializes_to_json() {
    let manifest = sample_manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("\"version\": 2"));
    assert!(json.contains("\"snp_launch_digest\": \"aabbcc\""));
    assert!(json.contains("\"vmsa_count\": 4"));
    assert!(json.contains("\"firmware\""));
    assert!(json.contains("\"base_image\""));
    assert!(json.contains("\"variants\""));
}

#[test]
fn manifest_roundtrip() {
    let manifest = sample_manifest();
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.version, manifest.version);
    assert_eq!(deserialized.variants.len(), 1);
    assert_eq!(deserialized.variants[0].smp, 4);
    assert_eq!(
        deserialized.variants[0].measurement.snp_launch_digest,
        "aabbcc"
    );
    assert_eq!(deserialized.inputs.initrd.path, "initrd.cpio.gz");
    assert_eq!(deserialized.outputs.disk_image.path, "disk.raw");
}

#[test]
fn manifest_roundtrip_multi_variant() {
    let mut manifest = sample_manifest();
    manifest.variants = vec![
        sample_variant(2, "digest2"),
        sample_variant(4, "digest4"),
        sample_variant(8, "digest8"),
    ];
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.variants.len(), 3);
    let digests: Vec<String> = deserialized
        .variants
        .iter()
        .map(|v| v.measurement.snp_launch_digest.clone())
        .collect();
    assert_eq!(digests, vec!["digest2", "digest4", "digest8"]);
}

#[test]
fn manifest_optional_fields_omitted() {
    let mut manifest = sample_manifest();
    manifest.inputs.firmware = None;
    manifest.variants.clear();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(!json.contains("firmware"));
    assert!(json.contains("\"variants\": []"));
}

#[test]
fn sha256_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.bin");
    fs_err::write(&path, b"hello world").unwrap();
    let hash = steep::manifest::sha256_file(&path).unwrap();
    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn parse_igvm_manifest() {
    let igvm_json = r#"{
        "measurement": {
            "snp_launch_digest": "aabbccdd",
            "algorithm": "sha384",
            "page_count": 5598,
            "vmsa_count": 4
        }
    }"#;
    let measurement = steep::manifest::parse_igvm_manifest(igvm_json).unwrap();
    assert_eq!(measurement.snp_launch_digest, "aabbccdd");
    assert_eq!(measurement.algorithm, "sha384");
    assert_eq!(measurement.page_count, 5598);
    assert_eq!(measurement.vmsa_count, 4);
}

#[test]
fn read_manifest_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    let manifest = sample_manifest();
    steep::manifest::write_manifest(&manifest, &path).unwrap();
    let loaded = steep::manifest::read_manifest(&path).unwrap();
    assert_eq!(loaded.variants[0].smp, 4);
    assert_eq!(loaded.build.memory, "2G");
    assert_eq!(loaded.build.format, "raw");
}

#[test]
fn manifest_rejects_unknown_fields_in_build() {
    let json = r#"{
        "version": 2,
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
        "variants": []
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
        "variants": [],
        "injected": "malicious"
    }"#;
    let result: Result<BuildManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "should reject unknown top-level fields");
}

#[test]
fn manifest_rejects_v1_legacy_fields() {
    // v1 carried `build.smp`, `outputs.igvm`, and a top-level `measurement`;
    // with deny_unknown_fields these must fail to parse so old manifests
    // can't be silently mis-read by a v2 consumer.
    let json = r#"{
        "version": 1,
        "build": {
            "timestamp": "2026-03-13T12:00:00Z",
            "smp": 4,
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
            "igvm": {"path": "g.igvm", "sha256": "h"},
            "uki": {"path": "g", "sha256": "h"}
        }
    }"#;
    let result: Result<BuildManifest, _> = serde_json::from_str(json);
    assert!(result.is_err(), "v1 manifest should not parse as v2");
}

#[test]
fn parse_igvm_manifest_missing_measurement_key() {
    let json = r#"{"other_key": "value"}"#;
    let result = steep::manifest::parse_igvm_manifest(json);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.to_string().contains("measurement"));
}

#[test]
fn parse_igvm_manifest_invalid_json() {
    let result = steep::manifest::parse_igvm_manifest("not json at all");
    assert!(result.is_err());
}

#[test]
fn parse_igvm_manifest_incomplete_measurement() {
    let json = r#"{
        "measurement": {
            "snp_launch_digest": "aabbcc"
        }
    }"#;
    let result = steep::manifest::parse_igvm_manifest(json);
    assert!(
        result.is_err(),
        "should reject measurement missing required fields"
    );
}

#[test]
fn sha256_file_nonexistent() {
    let result = steep::manifest::sha256_file(std::path::Path::new("/nonexistent/file.bin"));
    assert!(result.is_err());
}

#[test]
fn sha256_file_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.bin");
    fs_err::write(&path, b"").unwrap();
    let hash = steep::manifest::sha256_file(&path).unwrap();
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
    steep::manifest::write_manifest(&manifest, &path).unwrap();

    let content = fs_err::read_to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(value["version"], 2);
    assert_eq!(value["variants"][0]["smp"], 4);
}

#[test]
fn read_manifest_nonexistent_file() {
    let result = steep::manifest::read_manifest(std::path::Path::new("/nonexistent/manifest.json"));
    assert!(result.is_err());
}

#[test]
fn read_manifest_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    fs_err::write(&path, "not json").unwrap();
    let result = steep::manifest::read_manifest(&path);
    assert!(result.is_err());
}

#[test]
fn basename_of_strips_directories() {
    use std::path::Path;
    assert_eq!(
        steep::manifest::basename_of(Path::new("/abs/path/to/disk.raw")),
        "disk.raw"
    );
    assert_eq!(
        steep::manifest::basename_of(Path::new("relative/dir/OVMF.fd")),
        "OVMF.fd"
    );
    assert_eq!(
        steep::manifest::basename_of(Path::new("uki.efi")),
        "uki.efi"
    );
}
