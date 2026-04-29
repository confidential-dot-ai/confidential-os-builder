//! Fetch a kernel tarball from cdn.kernel.org and verify its SHA256.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::tools;

/// Download `linux-<version>.tar.xz` to `cache_dir`, verify SHA256.
/// Returns the path to the verified tarball.
/// Skips download if the cached file already passes the SHA check.
pub fn fetch(version: &str, expected_sha256: &str, cache_dir: &Path) -> Result<PathBuf> {
    fs_err::create_dir_all(cache_dir)?;
    let major_dir = major_dir_for(version)?;
    let url = format!(
        "https://cdn.kernel.org/pub/linux/kernel/{}/linux-{}.tar.xz",
        major_dir, version
    );
    let dest = cache_dir.join(format!("linux-{}.tar.xz", version));

    if dest.exists() {
        let actual = sha256_file(&dest)?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            tracing::info!(path = %dest.display(), "tarball cache hit");
            return Ok(dest);
        }
        tracing::warn!(path = %dest.display(), "cached tarball sha mismatch, re-downloading");
    }

    tracing::info!(%url, "fetching kernel tarball");
    tools::run_command_streaming(
        "curl",
        &[
            "--fail",
            "--show-error",
            "--silent",
            "--location",
            "--output",
            &dest.to_string_lossy(),
            &url,
        ],
    )
    .with_context(|| format!("downloading {}", url))?;

    let actual = sha256_file(&dest)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            "tarball SHA256 mismatch for {}:\n  expected {}\n  actual   {}",
            dest.display(),
            expected_sha256,
            actual
        ));
    }
    Ok(dest)
}

/// Compute SHA256 of a file as a lowercase hex string.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs_err::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

/// Map a kernel semver to the cdn.kernel.org subdirectory.
/// Versions 6.x.y → "v6.x"; versions 5.x.y → "v5.x"; etc.
fn major_dir_for(version: &str) -> Result<String> {
    let major = version
        .split('.')
        .next()
        .ok_or_else(|| anyhow!("malformed version: {}", version))?;
    Ok(format!("v{}.x", major))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_dir_for_v6() {
        assert_eq!(major_dir_for("6.12.7").unwrap(), "v6.x");
    }

    #[test]
    fn major_dir_for_v5() {
        assert_eq!(major_dir_for("5.15.0").unwrap(), "v5.x");
    }

    #[test]
    fn sha256_file_known_value() {
        let d = tempfile::TempDir::new().unwrap();
        let p = d.path().join("hello");
        fs_err::write(&p, b"hello").unwrap();
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_file(&p).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
