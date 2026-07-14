use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current manifest schema version. v3 splits SNP measurements (which vary
/// per vCPU count) into `snp_variants[]` and adds a singleton `tdx` block
/// of TDX reference measurements. The TDX block is SMP-and-memory-invariant
/// thanks to the trusted-DSDT override mechanism that strips the only AML
/// fields that varied per (vCPU, memory) topology. v2 manifests fail to
/// parse — there is no reader compatibility shim.
pub const MANIFEST_VERSION: u32 = 3;

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BuildManifest {
    pub version: u32,
    pub build: BuildConfig,
    pub inputs: ManifestInputs,
    pub outputs: ManifestOutputs,
    /// Per-SMP SNP IGVM variants. One entry per vCPU count built; populated
    /// by `confos build` and extended/replaced in place by `confos igvm`.
    /// Omitted from JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snp_variants: Vec<SnpVariant>,
    /// TDX reference measurements — single block, SMP-and-memory-invariant
    /// thanks to the trusted-DSDT override that strips the AML fields that
    /// varied across topologies. `None` when the build excluded TDX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdx: Option<TdxMeasurement>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    pub timestamp: String,
    pub memory: String,
    pub format: String,
    pub platform: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SnpVariant {
    pub smp: u32,
    pub igvm: FileEntry,
    pub measurement: Measurement,
}

/// TDX reference measurements for the platform.
///
/// Hex-encoded SHA-384 digests (96 lowercase hex chars each, 48 bytes).
///
/// - `mrtd`  — MRTD computed from the OVMF/TDVF binary by simulating the
///   TDX Module's `MEM.PAGE.ADD` + `MR.EXTEND` algorithm. Fixed per
///   firmware build.
/// - `rtmr1` — Authenticode hash of the UKI PE image plus boot-service
///   constants and (when a disk image is supplied) the GPT header event.
/// - `rtmr2` — UKI section measurement chain (.linux, .osrel, .cmdline,
///   .initrd, plus the systemd-stub assembled-initrd Event 14).
///
/// Deliberately absent: `rtmr0`. RTMR[0] mixes TD-HOB (memory-sensitive)
/// and ACPI tables (memory + SMP-sensitive) coming from the VMM. Pinning
/// it would force a per-`(smp × memory)` matrix of manifest variants.
/// We avoid that by overriding the VMM-supplied DSDT with our trusted
/// AML via the initrd, then attesting the override through RTMR[2] /
/// the IGVM launch digest.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TdxMeasurement {
    pub mrtd: String,
    pub rtmr1: String,
    pub rtmr2: String,
    /// File entry for the TDX firmware binary the TDX measurements were
    /// computed against. Different from `inputs.firmware` (which records
    /// the SNP-side IGVM-aware firmware): TDX needs TDVF support that
    /// confos's edk2 fork does not include, so a both-platform build
    /// uses two distinct OVMF binaries. `Option` because older
    /// manifests written before the dual-firmware split land here as None.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub firmware: Option<FileEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ManifestInputs {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kernel: Option<KernelInputs>,
    pub initrd: FileEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub firmware: Option<FileEntry>,
    pub base_image: FileEntry,
}

#[derive(Serialize, Deserialize, Debug)]
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ManifestOutputs {
    pub disk_image: FileEntry,
    pub uki: FileEntry,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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

/// Read a manifest from a JSON file. Rejects any `version` other than
/// [`MANIFEST_VERSION`] before attempting field-by-field deserialization,
/// so a v2 manifest fails with a clear error instead of a confusing
/// "missing field" complaint.
pub fn read_manifest(path: &Path) -> anyhow::Result<BuildManifest> {
    let content = fs_err::read_to_string(path)?;
    // Peek at just `version` first. v2 readers don't exist outside this
    // codebase so we don't migrate — just refuse and tell the operator.
    let probe: serde_json::Value = serde_json::from_str(&content)?;
    if let Some(v) = probe.get("version").and_then(|v| v.as_u64()) {
        if v != u64::from(MANIFEST_VERSION) {
            anyhow::bail!(
                "manifest at {} is version {} (this build of confos speaks v{}). Rebuild with the current confos.",
                path.display(),
                v,
                MANIFEST_VERSION
            );
        }
    }
    let manifest: BuildManifest = serde_json::from_str(&content)?;
    Ok(manifest)
}
