//! Parse the `kernel/version` pin file.

use anyhow::{anyhow, Result};

/// Parsed contents of `kernel/version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelVersion {
    pub linux_version: String,
    pub tarball_sha256: String,
}

impl KernelVersion {
    /// Parse a KEY=VALUE blob (one per line; `#` comments and blank lines OK).
    pub fn parse(s: &str) -> Result<Self> {
        // Implemented in B2.
        let _ = s;
        Err(anyhow!("not yet implemented"))
    }
}
