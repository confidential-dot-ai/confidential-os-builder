use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
pub struct BuildManifest {
    pub version: u32,
    pub build: BuildConfig,
    pub inputs: ManifestInputs,
    pub outputs: ManifestOutputs,
    pub measurement: Measurement,
}

#[derive(Serialize, Deserialize)]
pub struct BuildConfig {
    pub timestamp: String,
    pub smp: u32,
    pub format: String,
    pub platform: String,
}

#[derive(Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
pub struct ManifestInputs {
    pub kernel: FileEntry,
    pub initrd: FileEntry,
    pub firmware: FileEntry,
    pub base_image: FileEntry,
    pub project_partition: FileEntry,
}

#[derive(Serialize, Deserialize)]
pub struct ManifestOutputs {
    pub disk_image: FileEntry,
    pub igvm: FileEntry,
    pub uki: FileEntry,
}

#[derive(Serialize, Deserialize)]
pub struct Measurement {
    pub snp_launch_digest: String,
    pub algorithm: String,
    pub page_count: u64,
    pub vmsa_count: u32,
}

/// Compute SHA-256 hash of a file, returned as a hex string.
/// Uses streaming reads to handle large files (disk images can be multiple GB).
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let result = hasher.finalize();
    Ok(hex::encode(result))
}

/// Write the manifest to a JSON file.
pub fn write_manifest(manifest: &BuildManifest, path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    fs_err::write(path, json)?;
    Ok(())
}
