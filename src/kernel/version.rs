//! Parse the `kernel/version` pin file.

use anyhow::{anyhow, Context, Result};

/// Parsed contents of `kernel/version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelVersion {
    pub linux_version: String,
    pub tarball_sha256: String,
}

impl KernelVersion {
    pub fn parse(s: &str) -> Result<Self> {
        let mut linux_version: Option<String> = None;
        let mut tarball_sha256: Option<String> = None;

        for (lineno, raw) in s.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| anyhow!("kernel/version: line {} malformed: {}", lineno + 1, raw))?;
            let value = v.trim().to_string();
            match k.trim() {
                "LINUX_VERSION" => linux_version = Some(value),
                "LINUX_TARBALL_SHA256" => tarball_sha256 = Some(value),
                other => return Err(anyhow!("kernel/version: unknown key '{}'", other)),
            }
        }

        Ok(KernelVersion {
            linux_version: linux_version
                .ok_or_else(|| anyhow!("kernel/version: missing LINUX_VERSION"))?,
            tarball_sha256: tarball_sha256
                .ok_or_else(|| anyhow!("kernel/version: missing LINUX_TARBALL_SHA256"))?,
        })
    }

    /// Read and parse `kernel/version` from disk.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let content =
            fs_err::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_valid_input() {
        let v =
            KernelVersion::parse("LINUX_VERSION=6.12.7\nLINUX_TARBALL_SHA256=abc123\n").unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc123");
    }

    #[test]
    fn ignores_blank_lines_and_comments() {
        let v = KernelVersion::parse(
            "# pinned version\n\nLINUX_VERSION=6.12.7\n# tarball hash\nLINUX_TARBALL_SHA256=abc\n",
        )
        .unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc");
    }

    #[test]
    fn fails_when_linux_version_missing() {
        let err = KernelVersion::parse("LINUX_TARBALL_SHA256=abc\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_VERSION"));
    }

    #[test]
    fn fails_when_sha_missing() {
        let err = KernelVersion::parse("LINUX_VERSION=6.12.7\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_TARBALL_SHA256"));
    }

    #[test]
    fn fails_on_unknown_key() {
        let err =
            KernelVersion::parse("LINUX_VERSION=1\nLINUX_TARBALL_SHA256=a\nBOGUS=x\n").unwrap_err();
        assert!(err.to_string().contains("BOGUS"));
    }

    #[test]
    fn trims_whitespace_around_value() {
        let v = KernelVersion::parse("LINUX_VERSION=  6.12.7  \nLINUX_TARBALL_SHA256=  abc  \n")
            .unwrap();
        assert_eq!(v.linux_version, "6.12.7");
        assert_eq!(v.tarball_sha256, "abc");
    }

    #[test]
    fn fails_on_malformed_line() {
        let err = KernelVersion::parse("LINUX_VERSION 6.12.7\n").unwrap_err();
        assert!(err.to_string().contains("LINUX_VERSION"));
    }
}
