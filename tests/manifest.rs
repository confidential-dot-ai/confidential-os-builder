use steep::manifest::{
    BuildManifest, BuildConfig, FileEntry, ManifestInputs, ManifestOutputs, Measurement,
};

fn sample_entry(path: &str) -> FileEntry {
    FileEntry { path: path.to_string(), sha256: "abc123".to_string() }
}

fn sample_manifest() -> BuildManifest {
    BuildManifest {
        version: 1,
        build: BuildConfig {
            timestamp: "2026-03-13T12:00:00Z".to_string(),
            smp: 4,
            memory: "2G".to_string(),
            format: "qcow2".to_string(),
            platform: "snp".to_string(),
        },
        inputs: ManifestInputs {
            kernel: sample_entry("vmlinuz"),
            initrd: Some(sample_entry("initrd.img")),
            firmware: sample_entry("OVMF.fd"),
            base_image: sample_entry("base.raw"),
            project_partition: sample_entry("project.raw"),
        },
        outputs: ManifestOutputs {
            disk_image: sample_entry("disk.qcow2"),
            igvm: sample_entry("guest.igvm"),
            uki: sample_entry("uki.efi"),
        },
        measurement: Measurement {
            snp_launch_digest: "aabbcc".to_string(),
            algorithm: "sha384".to_string(),
            page_count: 5598,
            vmsa_count: 4,
        },
    }
}

#[test]
fn test_manifest_serializes_to_json() {
    let manifest = sample_manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("\"version\": 1"));
    assert!(json.contains("\"snp_launch_digest\": \"aabbcc\""));
    assert!(json.contains("\"vmsa_count\": 4"));
    assert!(json.contains("\"kernel\""));
    assert!(json.contains("\"firmware\""));
    assert!(json.contains("\"base_image\""));
    assert!(json.contains("\"project_partition\""));
}

#[test]
fn test_manifest_roundtrip() {
    let manifest = sample_manifest();
    let json = serde_json::to_string(&manifest).unwrap();
    let deserialized: BuildManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.version, manifest.version);
    assert_eq!(deserialized.build.smp, manifest.build.smp);
    assert_eq!(deserialized.inputs.kernel.path, "vmlinuz");
    assert_eq!(deserialized.outputs.disk_image.path, "disk.qcow2");
}

#[test]
fn test_sha256_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.bin");
    fs_err::write(&path, b"hello world").unwrap();
    let hash = steep::manifest::sha256_file(&path).unwrap();
    assert_eq!(hash, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
}

#[test]
fn test_parse_igvm_manifest() {
    let igvm_json = r#"{
        "snp_launch_digest": "aabbccdd",
        "algorithm": "sha384",
        "page_count": 5598,
        "vmsa_count": 4
    }"#;
    let measurement = steep::manifest::parse_igvm_manifest(igvm_json).unwrap();
    assert_eq!(measurement.snp_launch_digest, "aabbccdd");
    assert_eq!(measurement.algorithm, "sha384");
    assert_eq!(measurement.page_count, 5598);
    assert_eq!(measurement.vmsa_count, 4);
}

#[test]
fn test_manifest_includes_memory() {
    let manifest = sample_manifest();
    let json = serde_json::to_string_pretty(&manifest).unwrap();
    assert!(json.contains("\"memory\": \"2G\""));
}

#[test]
fn test_read_manifest_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    let manifest = sample_manifest();
    steep::manifest::write_manifest(&manifest, &path).unwrap();
    let loaded = steep::manifest::read_manifest(&path).unwrap();
    assert_eq!(loaded.build.smp, 4);
    assert_eq!(loaded.build.memory, "2G");
    assert_eq!(loaded.build.format, "qcow2");
}
