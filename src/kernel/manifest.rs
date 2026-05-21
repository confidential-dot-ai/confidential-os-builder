//! `output/kernel/manifest.json` schema and fingerprint helpers.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KernelManifest {
    pub version: u32,
    pub linux_version: String,
    pub inputs: Fingerprint,
    pub outputs: Outputs,
    pub built_at: String,
}

/// Canonical fingerprint of all inputs that determine the kernel build output.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub linux_version: String,
    pub tarball_sha256: String,
    pub required_config_sha256: String,
    pub hardening_config_sha256: String,
    // `default` so older manifests (pre-container.config) still deserialize
    // as a cache MISS rather than a parse error — the empty default won't
    // match a current build's hash, so the kernel rebuilds and the manifest
    // is overwritten with the new field populated.
    #[serde(default)]
    pub container_config_sha256: String,
    pub snapshot_config_sha256: String,
    pub tools_tree_digest: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Outputs {
    pub vmlinuz_sha256: String,
}

impl Fingerprint {
    /// Render this fingerprint as canonical JSON: keys sorted, no whitespace.
    /// Used to compare fingerprints across runs.
    pub fn to_canonical_json(&self) -> String {
        let mut m: BTreeMap<&str, &str> = BTreeMap::new();
        m.insert("linux_version", &self.linux_version);
        m.insert("tarball_sha256", &self.tarball_sha256);
        m.insert("required_config_sha256", &self.required_config_sha256);
        m.insert("hardening_config_sha256", &self.hardening_config_sha256);
        m.insert("container_config_sha256", &self.container_config_sha256);
        m.insert("snapshot_config_sha256", &self.snapshot_config_sha256);
        m.insert("tools_tree_digest", &self.tools_tree_digest);
        serde_json::to_string(&m).expect("BTreeMap of strings serializes")
    }
}

pub fn read(path: &Path) -> Result<KernelManifest> {
    let s = fs_err::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let m: KernelManifest =
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(m)
}

pub fn write(path: &Path, manifest: &KernelManifest) -> Result<()> {
    let s = serde_json::to_string_pretty(manifest)?;
    fs_err::write(path, s).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fp() -> Fingerprint {
        Fingerprint {
            linux_version: "6.12.7".into(),
            tarball_sha256: "a".repeat(64),
            required_config_sha256: "b".repeat(64),
            hardening_config_sha256: "c".repeat(64),
            container_config_sha256: "f".repeat(64),
            snapshot_config_sha256: "d".repeat(64),
            tools_tree_digest: "e".repeat(64),
        }
    }

    #[test]
    fn fingerprint_canonical_json_keys_sorted() {
        let json = sample_fp().to_canonical_json();
        // BTreeMap renders alphabetically: container, hardening, linux, required,
        // snapshot, tarball, tools.
        let c = json.find("container_config_sha256").unwrap();
        let h = json.find("hardening_config_sha256").unwrap();
        let l = json.find("linux_version").unwrap();
        let r = json.find("required_config_sha256").unwrap();
        let s = json.find("snapshot_config_sha256").unwrap();
        let t = json.find("tarball_sha256").unwrap();
        let tt = json.find("tools_tree_digest").unwrap();
        assert!(c < h && h < l && l < r && r < s && s < t && t < tt);
    }

    #[test]
    fn fingerprint_default_container_config_sha_for_legacy_manifests() {
        // Older manifests written before container.config existed omit the
        // field. `#[serde(default)]` should let them deserialize with empty,
        // which then forces a cache MISS rather than a parse error.
        let legacy = r#"{
            "linux_version": "6.12.7",
            "tarball_sha256": "aa",
            "required_config_sha256": "bb",
            "hardening_config_sha256": "cc",
            "snapshot_config_sha256": "dd",
            "tools_tree_digest": "ee"
        }"#;
        let fp: Fingerprint = serde_json::from_str(legacy).unwrap();
        assert_eq!(fp.container_config_sha256, "");
        assert_eq!(fp.linux_version, "6.12.7");
    }

    #[test]
    fn fingerprint_canonical_json_no_whitespace() {
        let json = sample_fp().to_canonical_json();
        assert!(!json.contains(' '));
        assert!(!json.contains('\n'));
    }

    #[test]
    fn fingerprint_round_trips_via_serde() {
        let fp = sample_fp();
        let s = serde_json::to_string(&fp).unwrap();
        let back: Fingerprint = serde_json::from_str(&s).unwrap();
        assert_eq!(fp, back);
    }

    #[test]
    fn equal_fingerprints_render_equal_json() {
        let a = sample_fp();
        let b = sample_fp();
        assert_eq!(a.to_canonical_json(), b.to_canonical_json());
    }

    #[test]
    fn changing_one_field_changes_json() {
        let mut a = sample_fp();
        let b = a.clone();
        a.linux_version = "6.12.8".into();
        assert_ne!(a.to_canonical_json(), b.to_canonical_json());
    }

    #[test]
    fn read_parses_golden_fixture() {
        let path = std::path::Path::new("tests/fixtures/kernel_manifest.json");
        let m = read(path).unwrap();
        assert_eq!(m.version, 1);
        assert_eq!(m.linux_version, "6.12.7");
        assert_eq!(m.outputs.vmlinuz_sha256.len(), 64);
    }
}
