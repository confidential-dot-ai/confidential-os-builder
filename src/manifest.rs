use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current manifest schema version. v2 introduces `variants[]` (one entry per
/// SMP configuration of the IGVM) and removes the singleton `outputs.igvm` and
/// top-level `measurement` fields. v1 manifests fail to parse — there is no
/// reader compatibility shim.
pub const MANIFEST_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildManifest {
    pub version: u32,
    pub build: BuildConfig,
    pub inputs: ManifestInputs,
    pub outputs: ManifestOutputs,
    /// One entry per (SMP) IGVM variant. Populated by `steep build` (initial
    /// variant) and extended/replaced in place by `steep igvm`.
    #[serde(default)]
    pub variants: Vec<IgvmVariant>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    pub timestamp: String,
    pub memory: String,
    pub format: String,
    pub platform: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct IgvmVariant {
    pub smp: u32,
    pub igvm: FileEntry,
    pub measurement: Measurement,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInputs {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kernel: Option<KernelInputs>,
    pub initrd: FileEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FileEntry>,
    pub base_image: FileEntry,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KernelInputs {
    pub linux_version: String,
    pub vmlinuz_sha256: String,
    pub required_config_sha256: String,
    pub hardening_config_sha256: String,
    // `default` for backwards compat with manifests written before the
    // kernel-extra fragment field existed; see Fingerprint in
    // src/kernel/manifest.rs for the same trade-off explained at the source.
    #[serde(default)]
    pub kernel_extra_config_sha256: String,
    pub snapshot_config_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestOutputs {
    pub disk_image: FileEntry,
    pub uki: FileEntry,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub snp_launch_digest: String,
    pub algorithm: String,
    pub page_count: u64,
    pub vmsa_count: u32,
}

/// Extract the file basename from a path as an owned String.
/// Manifest entries record only the basename so they're portable across hosts.
pub fn basename_of(path: &Path) -> String {
    path.file_name()
        .expect("manifest paths must have a file name component")
        .to_string_lossy()
        .into_owned()
}

/// Compute SHA-256 hash of a file, returned as a hex string.
/// Uses streaming reads to handle large files (disk images can be multiple GB).
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Write the manifest to a JSON file.
pub fn write_manifest(manifest: &BuildManifest, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    fs_err::write(path, json)?;
    Ok(())
}

/// Parse the igvm-tools manifest JSON to extract measurement data.
/// The igvm-tools manifest nests the measurement under a "measurement" key.
pub fn parse_igvm_manifest(json: &str) -> anyhow::Result<Measurement> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let measurement_value = value
        .get("measurement")
        .ok_or_else(|| anyhow::anyhow!("igvm manifest missing 'measurement' key"))?;
    let measurement: Measurement = serde_json::from_value(measurement_value.clone())?;
    Ok(measurement)
}

/// Read a manifest from a JSON file.
pub fn read_manifest(path: &Path) -> anyhow::Result<BuildManifest> {
    let content = fs_err::read_to_string(path)?;
    let manifest: BuildManifest = serde_json::from_str(&content)?;
    Ok(manifest)
}
