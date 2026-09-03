//! Integrity Policy Enforcement: the kernel executes code only from the
//! initramfs and from the dm-verity root whose hash its policy names.
//!
//! The policy is compiled into the kernel (`CONFIG_IPE_BOOT_POLICY`), and the
//! root hash is only known once the root partition exists, so `confos build`
//! runs mkosi once with the cached kernel, appends that hash to the committed
//! `kernel/ipe-boot-policy`, relinks the kernel in its build tree, and runs
//! mkosi again with the sealed kernel. The kernel is excluded from the root
//! partition (`mkosi/base/mkosi.repart/10-root.conf`), so both passes produce
//! the same root hash; the build fails if they do not.

use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// Committed policy every kernel is built with; sealing appends to it.
pub const BOOT_POLICY: &str = "kernel/ipe-boot-policy";
/// Where the policy is staged in the kernel tree. `kernel/hardening.config`
/// points `CONFIG_IPE_BOOT_POLICY` here, so it is part of the interface.
pub const STAGED_BOOT_POLICY: &str = "security/ipe/confos-boot-policy";
/// The repart definition that keeps the kernel out of the root partition.
pub const ROOT_REPART: &str = "mkosi/base/mkosi.repart/10-root.conf";

/// Whether the lineage's resolved kernel config has IPE built in.
pub fn enabled(snapshot: &Path) -> Result<bool> {
    let config = fs_err::read_to_string(snapshot)
        .with_context(|| format!("reading kernel config snapshot {}", snapshot.display()))?;
    Ok(config
        .lines()
        .any(|line| line.trim() == "CONFIG_SECURITY_IPE=y"))
}

/// The sealed policy: the committed base plus rules allowing execution,
/// module loading, and firmware loading from the verity volume with this
/// root hash. Anything else stays at the base's `DEFAULT action=DENY`.
pub fn seal(base: &str, roothash: &str) -> Result<String> {
    let digest = digest_name(roothash)?;
    let mut policy = base.to_string();
    if !policy.ends_with('\n') {
        policy.push('\n');
    }
    policy.push_str("# Sealed by `confos build` to the dm-verity root of this image.\n");
    for op in ["EXECUTE", "KMODULE", "FIRMWARE"] {
        policy.push_str(&format!(
            "op={op} dmverity_roothash={digest}:{roothash} action=ALLOW\n"
        ));
    }
    Ok(policy)
}

/// IPE names the digest the way the dm-verity target reports it; mkosi's
/// verity partitions use the default algorithm for their hash length.
fn digest_name(roothash: &str) -> Result<&'static str> {
    if !roothash
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(anyhow!("roothash is not lowercase hex: {roothash:?}"));
    }
    match roothash.len() {
        64 => Ok("sha256"),
        96 => Ok("sha384"),
        128 => Ok("sha512"),
        n => Err(anyhow!(
            "roothash has {n} hex chars, expected 64, 96, or 128"
        )),
    }
}

/// The root partition must exclude the kernel at the version being built, or
/// sealing would change the hash it seals. Checked before the first mkosi
/// pass so a kernel bump that forgot the repart file fails in seconds.
pub fn verify_kernel_excluded_from_root(repart: &Path, linux_version: &str) -> Result<()> {
    let text = fs_err::read_to_string(repart)?;
    let wanted = format!("ExcludeFiles=/usr/lib/modules/{linux_version}/vmlinuz");
    if text.lines().any(|line| line.trim() == wanted) {
        return Ok(());
    }
    Err(anyhow!(
        "{} must contain `{wanted}`: the kernel is sealed to the root hash, so the \
         root partition must not contain the kernel (update the line for the version \
         in kernel/version)",
        repart.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn seal_appends_allow_rules_for_each_kernel_read_op() {
        let sealed = seal(
            "policy_name=confos policy_version=0.0.0\nDEFAULT action=DENY",
            HASH,
        )
        .unwrap();
        assert!(
            sealed.starts_with("policy_name=confos policy_version=0.0.0\nDEFAULT action=DENY\n")
        );
        for op in ["EXECUTE", "KMODULE", "FIRMWARE"] {
            assert!(sealed.contains(&format!(
                "op={op} dmverity_roothash=sha256:{HASH} action=ALLOW\n"
            )));
        }
        assert!(!sealed.contains("KEXEC"));
    }

    #[test]
    fn seal_names_the_digest_by_hash_length() {
        assert!(seal("h\n", &"a".repeat(96)).unwrap().contains("sha384:"));
        assert!(seal("h\n", &"a".repeat(128)).unwrap().contains("sha512:"));
        assert!(seal("h\n", &"a".repeat(65)).is_err());
        assert!(seal("h\n", &"A".repeat(64)).is_err());
        assert!(seal("h\n", &"g".repeat(64)).is_err());
    }

    #[test]
    fn committed_policy_seals_and_denies_by_default() {
        let base = fs_err::read_to_string(repo(BOOT_POLICY)).unwrap();
        let rules: Vec<&str> = base
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(rules[0], "policy_name=confos policy_version=0.0.0");
        assert!(rules.contains(&"DEFAULT action=DENY"));
        assert!(rules.contains(&"op=EXECUTE boot_verified=TRUE action=ALLOW"));
        assert!(
            !base.contains("dmverity_roothash"),
            "the base must not name a root"
        );
        assert!(seal(&base, HASH).unwrap().ends_with("action=ALLOW\n"));
    }

    #[test]
    fn hardening_config_stages_the_policy_where_confos_writes_it() {
        let config = fs_err::read_to_string(repo("kernel/hardening.config")).unwrap();
        assert!(config.contains(&format!("CONFIG_IPE_BOOT_POLICY=\"{STAGED_BOOT_POLICY}\"")));
        assert!(config.contains("CONFIG_SECURITY_IPE=y"));
    }

    #[test]
    fn root_partition_excludes_the_pinned_kernel_version() {
        let version = fs_err::read_to_string(repo("kernel/version")).unwrap();
        let linux_version = version
            .lines()
            .find_map(|l| l.strip_prefix("LINUX_VERSION="))
            .unwrap()
            .trim();
        verify_kernel_excluded_from_root(&repo(ROOT_REPART), linux_version).unwrap();
        let err = verify_kernel_excluded_from_root(&repo(ROOT_REPART), "0.0.0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ExcludeFiles=/usr/lib/modules/0.0.0/vmlinuz"));
    }

    #[test]
    fn enabled_reads_the_resolved_symbol() {
        let d = tempfile::TempDir::new().unwrap();
        let on = d.path().join("on");
        fs_err::write(&on, "CONFIG_SECURITYFS=y\nCONFIG_SECURITY_IPE=y\n").unwrap();
        assert!(enabled(&on).unwrap());
        let off = d.path().join("off");
        fs_err::write(&off, "# CONFIG_SECURITY_IPE is not set\n").unwrap();
        assert!(!enabled(&off).unwrap());
        assert!(enabled(&d.path().join("missing")).is_err());
    }

    fn repo(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
    }
}
