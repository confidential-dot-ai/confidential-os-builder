//! Mandatory trusted-AML kernel inputs and resolved-config checks.
//!
//! The source table and the single, version-specific kernel patch are both
//! cache inputs. The kernel enforces provenance before AML namespace parsing.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::config;

pub const DSDT_SOURCE: &str = "kernel/trusted-dsdt.asl";
pub const PATCH: &str = "kernel/patches/0001-acpi-trusted-aml.patch";
pub const HEADER: &str = "confos-trusted-dsdt.h";
const STAGED_DSDT: &str = "confos-trusted-dsdt.asl";
const STAGED_PATCH: &str = "confos-trusted-aml.patch";
/// Trailer in the patch header naming the kernel its hooks were audited on.
const VERSION_TRAILER: &str = "Linux-Version:";
/// Applies the patch, run from the tree root inside the pinned tools tree.
/// Every name is a fixed builder-owned string; no caller text reaches the
/// shell.
const PATCH_SCRIPT: &str = "set -eu\n\
    cd /build\n\
    patch --batch --forward --fuzz=0 -p1 < confos-trusted-aml.patch\n";
/// Compiles an ASL table into the header, after [`PATCH_SCRIPT`].
const COMPILE_SCRIPT: &str = "iasl -ve -tc -p dsdt confos-trusted-dsdt.asl\n\
    grep -q 'unsigned char dsdt_aml_code\\[\\]' dsdt.hex\n\
    cp dsdt.hex include/confos-trusted-dsdt.h\n";

/// Resolved `.config` lines that must hold once every fragment has merged.
/// The header path is checked as well as the booleans: selecting another
/// valid built-in DSDT would otherwise satisfy Kconfig while changing our
/// contract.
const REQUIRED: [(&str, &str); 4] = [
    ("CONFIG_ACPI", "y"),
    ("CONFIG_ACPI_CUSTOM_DSDT", "y"),
    ("CONFIG_ACPI_TRUSTED_AML", "y"),
    ("CONFIG_ACPI_CUSTOM_DSDT_FILE", "\"confos-trusted-dsdt.h\""),
];
/// Alternate table loaders that must stay off (`=y` and `=m` both fail).
const FORBIDDEN: [&str; 4] = [
    "CONFIG_ACPI_TABLE_UPGRADE",
    "CONFIG_ACPI_CONFIGFS",
    "CONFIG_EFI_CUSTOM_SSDT_OVERLAYS",
    "CONFIG_ACPI_DEBUGGER",
];

/// The patch declares the one kernel it was audited against; `kernel/version`
/// must match it. A pin bump therefore fails until the loader hooks are
/// re-audited and the patch retargeted, and each pin has exactly one home.
pub fn verify_version(version: &str) -> Result<()> {
    let patch = fs_err::read_to_string(PATCH)?;
    let supported = supported_version(&patch)?;
    if version != supported {
        bail!(
            "{PATCH} supports Linux {supported}, got {version}; re-audit the ACPI loader \
             hooks, retarget the patch and update its {VERSION_TRAILER} trailer"
        );
    }
    Ok(())
}

fn supported_version(patch: &str) -> Result<&str> {
    // Only the header carries the trailer; the diff body could quote it.
    let header = patch
        .split_once("\n---\n")
        .map_or(patch, |(header, _)| header);
    let mut versions = header
        .lines()
        .filter_map(|line| line.strip_prefix(VERSION_TRAILER))
        .map(str::trim)
        .filter(|v| !v.is_empty());
    match (versions.next(), versions.next()) {
        (Some(v), None) => Ok(v),
        (None, _) => bail!("{PATCH} has no {VERSION_TRAILER} trailer in its header"),
        (Some(_), Some(_)) => bail!("{PATCH} has more than one {VERSION_TRAILER} trailer"),
    }
}

/// Copy the table and the patch into the freshly extracted kernel tree,
/// then apply the patch and compile the table inside the pinned tools tree.
/// A raw `.aml` table is embedded byte for byte instead of compiled.
/// Runs before Kconfig sees the tree, so the patch's new symbol resolves.
pub fn stage(tools_tree: &Path, kernel_src: &Path, dsdt: &Path) -> Result<()> {
    fs_err::copy(PATCH, kernel_src.join(STAGED_PATCH))?;
    if dsdt.extension().is_some_and(|e| e == "aml") {
        fs_err::write(
            kernel_src.join("include").join(HEADER),
            aml_header(&fs_err::read(dsdt)?)?,
        )?;
        return config::nspawn(tools_tree, kernel_src, "/build", &[], PATCH_SCRIPT);
    }
    fs_err::copy(dsdt, kernel_src.join(STAGED_DSDT))?;
    let script = format!("{PATCH_SCRIPT}{COMPILE_SCRIPT}");
    config::nspawn(tools_tree, kernel_src, "/build", &[], &script)
}

/// C header in iasl's `-tc` shape for a captured binary DSDT.
fn aml_header(table: &[u8]) -> Result<String> {
    if table.len() < 36 || &table[..4] != b"DSDT" {
        bail!("trusted table is not a DSDT");
    }
    if table.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) != 0 {
        bail!("trusted DSDT has a bad checksum");
    }
    let mut out = String::from("unsigned char dsdt_aml_code[] =\n{\n");
    for chunk in table.chunks(16) {
        out.push_str("   ");
        for b in chunk {
            write!(out, " 0x{b:02X},")?;
        }
        out.push('\n');
    }
    out.push_str("};\n");
    Ok(out)
}

/// Fragment line that selects the ACPI policy for a whole kernel lineage.
const MODE_DIRECTIVE: &str = "# confos-acpi:";

/// Which AML the kernel may execute. Chosen per lineage by a directive line
/// in the caller's fragment, so the choice is part of the fingerprinted input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Default: only the built-in `kernel/trusted-dsdt.asl`.
    #[default]
    Trusted,
    /// `# confos-acpi: host` — the firmware's DSDT/SSDTs run unchanged. Only
    /// for cloud profiles whose host ACPI layout confos cannot pin; the
    /// tables must instead be checked at attestation time (TDX: RTMR[0]
    /// via the CCEL event log). Runtime table loaders stay forbidden.
    Host,
}

impl Mode {
    /// Manifests omit the default mode, so default builds keep their JSON.
    pub fn is_trusted(&self) -> bool {
        *self == Mode::Trusted
    }
}

/// The caller fragment's directive split into words; `None` when absent.
fn directive(fragment: Option<&str>) -> Result<Option<Vec<&str>>> {
    let mut lines = fragment
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().strip_prefix(MODE_DIRECTIVE));
    let words = lines.next().map(|l| l.split_whitespace().collect());
    if lines.next().is_some() {
        bail!("more than one {MODE_DIRECTIVE} line in the kernel fragment");
    }
    Ok(words)
}

/// Read the ACPI mode from the caller's fragment; absent means `Trusted`.
pub fn mode(fragment: Option<&str>) -> Result<Mode> {
    Ok(match directive(fragment)?.as_deref() {
        None | Some(["trusted"] | ["trusted", _]) => Mode::Trusted,
        Some(["host"]) => Mode::Host,
        Some(other) => {
            bail!("invalid {MODE_DIRECTIVE} {other:?}; expected `trusted [TABLE]` or `host`")
        }
    })
}

/// The table compiled into the kernel: `# confos-acpi: trusted TABLE`
/// selects a lineage-specific one (repo-relative `.asl` source, or a raw
/// `.aml` table captured from the target platform), else the default.
pub fn dsdt_source(fragment: Option<&Path>) -> Result<PathBuf> {
    let text = fragment.map(fs_err::read_to_string).transpose()?;
    mode(text.as_deref())?;
    Ok(match directive(text.as_deref())?.as_deref() {
        Some(["trusted", table]) => PathBuf::from(table),
        _ => PathBuf::from(DSDT_SOURCE),
    })
}

/// These invariants apply after all consumer fragments have been merged.
pub fn verify_config(config: &str, mode: Mode) -> Result<()> {
    let values = config::values(config);
    if mode == Mode::Host {
        for symbol in ["CONFIG_ACPI_TRUSTED_AML", "CONFIG_ACPI_CUSTOM_DSDT"] {
            if values.contains_key(symbol) {
                bail!("{MODE_DIRECTIVE} host requires {symbol} off; the host DSDT must not be replaced");
            }
        }
        if values.get("CONFIG_ACPI") != Some(&"y") {
            bail!("{MODE_DIRECTIVE} host requires CONFIG_ACPI=y");
        }
        return verify_no_loaders(&values);
    }
    for (symbol, value) in REQUIRED {
        if values.get(symbol) != Some(&value) {
            bail!("trusted AML requires resolved {symbol}={value}; consumer fragments cannot disable this boundary");
        }
    }
    verify_no_loaders(&values)
}

fn verify_no_loaders(values: &std::collections::HashMap<&str, &str>) -> Result<()> {
    for symbol in FORBIDDEN {
        if matches!(values.get(symbol), Some(&"y" | &"m")) {
            bail!("trusted AML forbids {symbol}; alternate table loaders must remain disabled");
        }
    }
    Ok(())
}

/// A resolved `.config` excerpt satisfying [`verify_config`]; shared with
/// the config tests so a new requirement is added in one place.
#[cfg(test)]
pub(crate) const VALID_CONFIG: &str = "CONFIG_ACPI=y\nCONFIG_ACPI_CUSTOM_DSDT=y\nCONFIG_ACPI_TRUSTED_AML=y\nCONFIG_ACPI_CUSTOM_DSDT_FILE=\"confos-trusted-dsdt.h\"\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_script_names_the_staged_inputs_and_header() {
        assert!(PATCH_SCRIPT.ends_with(&format!("-p1 < {STAGED_PATCH}\n")));
        assert!(COMPILE_SCRIPT.contains(&format!("-p dsdt {STAGED_DSDT}\n")));
        assert!(COMPILE_SCRIPT.ends_with(&format!("include/{HEADER}\n")));
    }

    #[test]
    fn rejects_weakened_consumer_configuration() {
        verify_config(VALID_CONFIG, Mode::Trusted).unwrap();
        for required in VALID_CONFIG.lines() {
            let weakened = VALID_CONFIG.replace(required, "");
            assert!(
                verify_config(&weakened, Mode::Trusted).is_err(),
                "accepted missing {required}"
            );
        }
        assert!(
            verify_config(&VALID_CONFIG.replace(HEADER, "other-dsdt.h"), Mode::Trusted).is_err()
        );
        for loader in FORBIDDEN {
            for value in ["y", "m"] {
                assert!(
                    verify_config(&format!("{VALID_CONFIG}{loader}={value}\n"), Mode::Trusted)
                        .is_err()
                );
            }
        }
        // A disabled loader is fine; only enabled ones are forbidden.
        verify_config(
            &format!("{VALID_CONFIG}# CONFIG_ACPI_TABLE_UPGRADE is not set\n"),
            Mode::Trusted,
        )
        .unwrap();
    }

    #[test]
    fn captured_aml_is_embedded_byte_for_byte() {
        let mut table = b"DSDT".to_vec();
        table.extend((0..100u8).map(|b| b.wrapping_mul(37)));
        let sum = table.iter().fold(0u8, |s, b| s.wrapping_add(*b));
        table[9] = table[9].wrapping_sub(sum);
        let header = aml_header(&table).unwrap();
        let body = header
            .strip_prefix("unsigned char dsdt_aml_code[] =\n{\n")
            .and_then(|h| h.strip_suffix("};\n"))
            .unwrap();
        let embedded: Vec<u8> = body
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| u8::from_str_radix(t.trim_start_matches("0x"), 16).unwrap())
            .collect();
        assert_eq!(embedded, table);
        table[10] ^= 1;
        assert!(aml_header(&table).is_err(), "bad checksum accepted");
        assert!(aml_header(b"SSDT").is_err());
    }

    #[test]
    fn fragment_directive_selects_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let fragment = |text: &str| {
            let path = dir.path().join("f.config");
            fs_err::write(&path, text).unwrap();
            dsdt_source(Some(&path))
        };
        assert_eq!(dsdt_source(None).unwrap(), PathBuf::from(DSDT_SOURCE));
        assert_eq!(
            fragment("CONFIG_X=y\n").unwrap(),
            PathBuf::from(DSDT_SOURCE)
        );
        assert_eq!(
            fragment("# confos-acpi: host\n").unwrap(),
            PathBuf::from(DSDT_SOURCE)
        );
        assert_eq!(
            fragment("# confos-acpi: trusted kernel/acpi/x.aml\n").unwrap(),
            PathBuf::from("kernel/acpi/x.aml")
        );
        assert!(fragment("# confos-acpi: host kernel/acpi/x.aml\n").is_err());
    }

    #[test]
    fn host_mode_is_explicit_and_keeps_loaders_forbidden() {
        assert_eq!(mode(None).unwrap(), Mode::Trusted);
        assert_eq!(mode(Some("CONFIG_X=y\n")).unwrap(), Mode::Trusted);
        assert_eq!(mode(Some("# confos-acpi: host\n")).unwrap(), Mode::Host);
        assert!(mode(Some("# confos-acpi: off\n")).is_err());
        assert!(mode(Some("# confos-acpi: host\n# confos-acpi: host\n")).is_err());
        let host = "CONFIG_ACPI=y\n";
        verify_config(host, Mode::Host).unwrap();
        assert!(verify_config(host, Mode::Trusted).is_err());
        assert!(verify_config(VALID_CONFIG, Mode::Host).is_err());
        assert!(verify_config(&format!("{host}CONFIG_ACPI_CONFIGFS=y\n"), Mode::Host).is_err());
    }

    #[test]
    fn supported_version_comes_from_the_header_trailer_only() {
        let patch =
            "Subject: x\n\nLinux-Version: 6.18.49\n---\n--- a/f\n+++ b/f\n+Linux-Version: 9.9.9\n";
        assert_eq!(supported_version(patch).unwrap(), "6.18.49");
        assert!(supported_version("Subject: x\n---\n").is_err());
        assert!(supported_version("Linux-Version: 1\nLinux-Version: 2\n---\n").is_err());
        assert!(supported_version("Linux-Version:   \n---\n").is_err());
    }

    #[test]
    fn committed_patch_names_the_pinned_kernel() {
        let version =
            crate::kernel::version::KernelVersion::read(Path::new("kernel/version")).unwrap();
        verify_version(&version.linux_version).unwrap();
        let pinned = version.linux_version;
        for other in [
            "6.16.12",
            &format!("{pinned}0"),
            &format!("{pinned}-extra"),
            "",
        ] {
            assert!(verify_version(other).is_err(), "accepted {other:?}");
        }
    }
}
