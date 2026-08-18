use std::path::PathBuf;

fn repository_file(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs_err::read_to_string(root.join(path)).unwrap()
}

#[test]
fn attest_gpu_profile_enables_gpu_collection() {
    let config = repository_file(
        "mkosi/base/mkosi.profiles/attest-gpu/mkosi.extra/etc/attestation-api/config.toml",
    );
    assert_eq!(
        config
            .lines()
            .filter(|line| *line == "gpu_attestation_evidence_enabled = true")
            .count(),
        1
    );
    assert!(config.contains("platforms = [\"snp\", \"tdx\"]"));

    let service = repository_file(
        "mkosi/base/mkosi.profiles/attest-gpu/mkosi.extra/etc/systemd/system/attestation-api.service",
    );
    assert!(service.contains("-c /etc/attestation-api/config.toml"));
}

#[test]
fn gpu_image_paths_require_a_libnvat_binary() {
    let helper = repository_file("bin/confos-fetch-attest-gpu");
    assert!(helper.contains("readelf -d \"$ATTEST_GPU_BIN\""));
    assert!(helper.contains("Shared library: \\[libnvat\\.so"));

    let sync = repository_file("mkosi/base/mkosi.profiles/attest-gpu/mkosi.sync");
    assert!(sync.contains("readelf -d \"$1\""));
    assert!(sync.contains("Shared library: \\[libnvat\\.so"));
    assert_eq!(sync.matches("require_gpu_binary \"").count(), 2);

    let profile = repository_file("mkosi/base/mkosi.profiles/attest-gpu/mkosi.conf");
    assert!(profile.contains("ToolsTreePackages=oras,jq,binutils"));
}
