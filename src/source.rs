use std::path::{Path, PathBuf};

/// Check if a string looks like a URL.
pub fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://")
}

/// Extract the filename from a URL.
pub fn filename_from_url(url: &str) -> Option<String> {
    url.rsplit('/').next().map(|s| s.to_string())
}

/// Return the cache directory for downloaded base images.
pub fn cache_dir() -> PathBuf {
    dirs_path().join("base-inputs")
}

fn dirs_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()))
                .join(".local/share")
        });
    base.join("steep")
}

/// Resolve a source image string to a local path.
/// If the source is a URL, download it to the cache directory (skip if already cached).
/// If the source is a local path, validate it exists and return it.
pub fn resolve(source: &str) -> anyhow::Result<PathBuf> {
    if is_url(source) {
        let filename = filename_from_url(source)
            .ok_or_else(|| anyhow::anyhow!("cannot extract filename from URL: {source}"))?;
        let cache = cache_dir();
        fs_err::create_dir_all(&cache)?;
        let cached_path = cache.join(&filename);
        if cached_path.exists() {
            tracing::info!(path = %cached_path.display(), "using cached source image");
            return Ok(cached_path);
        }
        tracing::info!(url = source, dest = %cached_path.display(), "downloading source image");
        crate::tools::require("curl")?;
        let dest = cached_path.display().to_string();
        crate::tools::run_command_streaming("curl", &[
            "-fSL",
            "-o", &dest,
            source,
        ])?;
        Ok(cached_path)
    } else {
        let path = Path::new(source);
        if !path.exists() {
            anyhow::bail!("source image not found: {source}");
        }
        Ok(path.to_path_buf())
    }
}
