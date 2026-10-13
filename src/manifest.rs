use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::kernel::aml::Mode;

/// Current manifest schema version. v3 splits SNP measurements (which vary
/// per vCPU count) into `snp_variants[]` and adds a singleton `tdx` block
/// of TDX reference measurements. The TDX block is SMP-and-memory-invariant
/// because this policy excludes topology-dependent RTMR[0]. The kernel admits
/// only its built-in AML; non-AML host inputs remain a residual surface. v2 manifests fail to
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
    /// under the policy that excludes RTMR[0]. `None` when the build excluded TDX.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdx: Option<TdxMeasurement>,
    /// What cloud firmware measures for this disk and UKI. `None` in
    /// manifests written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudMeasurement>,
}

/// Expected registers of this image under cloud firmware, from
/// `tdx_measure::cloud` (docs/CLOUD.md, docs/MANIFEST.md). Firmware-owned
/// registers (MRTD, RTMR[0], PCR0-3/7) are not here: verifiers take them
/// from the cloud provider.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudMeasurement {
    /// `systemd-boot` or `direct-uki`: what `\EFI\BOOT\BOOTX64.EFI` is.
    pub boot_path: String,
    /// GCE TDX, SHA-384 hex.
    pub gcp_tdx: GcpTdx,
    /// Azure CVM vTPM SHA-256 bank, hex.
    pub azure_vtpm: AzureVtpm,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GcpTdx {
    pub rtmr1: String,
    pub rtmr2: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AzureVtpm {
    pub pcr4: String,
    pub pcr9: String,
    pub pcr11: String,
}

impl From<&tdx_measure::cloud::CloudMeasurement> for CloudMeasurement {
    fn from(m: &tdx_measure::cloud::CloudMeasurement) -> Self {
        CloudMeasurement {
            boot_path: m.boot_path.as_str().to_string(),
            gcp_tdx: GcpTdx {
                rtmr1: hex::encode(&m.gcp_tdx.rtmr1),
                rtmr2: hex::encode(&m.gcp_tdx.rtmr2),
            },
            azure_vtpm: AzureVtpm {
                pcr4: hex::encode(&m.azure_vtpm.pcr4),
                pcr9: hex::encode(&m.azure_vtpm.pcr9),
                pcr11: hex::encode(&m.azure_vtpm.pcr11),
            },
        }
    }
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
/// The measured kernel admits only its built-in DSDT before AML parsing.
/// Non-AML host data remains outside this policy. Kernel identity is in RTMR[2] /
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
    // Signing certificate built into the system keyring — a reproduction
    // input like the fragments. `default` for older manifests.
    #[serde(default)]
    pub module_signing_cert_sha256: String,
    // RANDSTRUCT seed — a reproduction input (#85). `default` for older manifests.
    #[serde(default)]
    pub randstruct_seed_sha256: String,
    /// `host` when the kernel runs the firmware's ACPI tables
    /// (`# confos-acpi: host`). Omitted for the default trusted mode.
    #[serde(default, skip_serializing_if = "Mode::is_trusted")]
    pub acpi_mode: Mode,
    /// SHA-256 of the trusted table compiled into the kernel. Required in
    /// trusted mode: a manifest without it comes from a builder whose kernel
    /// still honoured host AML, and reading it as a gated build would hide
    /// that. Absent in host mode, where no table is enforced
    /// ([`KernelInputs::check_acpi`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_dsdt_sha256: Option<String>,
    /// SHA-256 of the version-specific kernel enforcement patch. Required for
    /// the same reason.
    pub trusted_aml_patch_sha256: String,
}

impl KernelInputs {
    /// A trusted-mode kernel names its table; a host-mode kernel names none.
    pub fn check_acpi(&self) -> anyhow::Result<()> {
        match (self.acpi_mode, &self.trusted_dsdt_sha256) {
            (Mode::Trusted, Some(_)) | (Mode::Host, None) => Ok(()),
            (Mode::Trusted, None) => anyhow::bail!(
                "kernel inputs lack trusted_dsdt_sha256; the kernel may honour host AML"
            ),
            (Mode::Host, Some(_)) => {
                anyhow::bail!("kernel inputs name a trusted DSDT but acpi_mode is host")
            }
        }
    }
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
    if let Some(kernel) = &manifest.inputs.kernel {
        kernel.check_acpi()?;
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel_inputs(extra: serde_json::Value) -> anyhow::Result<KernelInputs> {
        let mut inputs = serde_json::json!({
            "linux_version": "6.18.49",
            "vmlinuz_sha256": "a",
            "required_config_sha256": "b",
            "hardening_config_sha256": "c",
            "snapshot_config_sha256": "d",
            "trusted_aml_patch_sha256": "f",
        });
        inputs
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let inputs: KernelInputs = serde_json::from_value(inputs)?;
        inputs.check_acpi()?;
        Ok(inputs)
    }

    #[test]
    fn kernel_inputs_require_the_trusted_aml_hashes() {
        kernel_inputs(serde_json::json!({"trusted_dsdt_sha256": "e"})).unwrap();
        assert!(
            kernel_inputs(serde_json::json!({})).is_err(),
            "accepted a manifest without trusted_dsdt_sha256"
        );
        let mut no_patch = serde_json::to_value(
            kernel_inputs(serde_json::json!({"trusted_dsdt_sha256": "e"})).unwrap(),
        )
        .unwrap();
        no_patch
            .as_object_mut()
            .unwrap()
            .remove("trusted_aml_patch_sha256");
        assert!(serde_json::from_value::<KernelInputs>(no_patch).is_err());
    }

    #[test]
    fn host_acpi_kernel_records_the_mode_and_no_table() {
        let host = kernel_inputs(serde_json::json!({"acpi_mode": "host"})).unwrap();
        let json = serde_json::to_value(&host).unwrap();
        assert_eq!(json["acpi_mode"], "host");
        assert!(json.get("trusted_dsdt_sha256").is_none());
        assert!(kernel_inputs(
            serde_json::json!({"acpi_mode": "host", "trusted_dsdt_sha256": "e"})
        )
        .is_err());
        // The default mode is not written, so default manifests keep their JSON.
        let trusted = kernel_inputs(serde_json::json!({"trusted_dsdt_sha256": "e"})).unwrap();
        assert!(serde_json::to_value(&trusted)
            .unwrap()
            .get("acpi_mode")
            .is_none());
    }

    /// The Azure 0.1.2 build manifest with the cloud block this builder adds
    /// (values as measured on Azure Standard_DC2es_v6).
    #[test]
    fn cloud_block_round_trips_and_is_optional() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/azure-cloud-manifest.json"
        );
        let manifest = read_manifest(Path::new(fixture)).unwrap();
        let cloud = manifest.cloud.as_ref().unwrap();
        assert_eq!(cloud.boot_path, "systemd-boot");
        assert_eq!(
            cloud.azure_vtpm.pcr4,
            "01a877a78619b163df80fb2cc469de09a74682efcc19b282e211285719cfa51a"
        );
        write_manifest(&manifest, &path).unwrap();
        let reread = read_manifest(&path).unwrap();
        assert_eq!(reread.cloud, manifest.cloud);

        let mut older: serde_json::Value =
            serde_json::from_str(&fs_err::read_to_string(fixture).unwrap()).unwrap();
        older.as_object_mut().unwrap().remove("cloud");
        fs_err::write(&path, older.to_string()).unwrap();
        assert!(read_manifest(&path).unwrap().cloud.is_none());
    }
}
