use std::ffi::OsString;

use crate::{tools, PullArgs};

pub fn run(args: &PullArgs) -> anyhow::Result<()> {
    tools::require("oras")?;

    let output_dir = match &args.dir {
        Some(dir) => dir.clone(),
        None => default_output_dir(&args.image).ok_or_else(|| {
            anyhow::anyhow!(
                "cannot derive an output directory from {} (no tag); pass a directory argument",
                args.image
            )
        })?,
    };

    if output_dir.exists() {
        anyhow::bail!(
            "output directory already exists: {}. Remove it first.",
            output_dir.display()
        );
    }

    fs_err::create_dir_all(&output_dir)?;

    println!("Pulling {} into {}", args.image, output_dir.display());

    let oras_args: Vec<OsString> = vec!["pull".into(), (&args.image).into()];

    let output_dir = output_dir
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to resolve output directory: {}", e))?;

    tools::run_command_streaming_in("oras", &oras_args, output_dir)?;

    println!("Pulled successfully.");
    Ok(())
}

/// Derive `output/<tag>` from an image reference like `ghcr.io/org/name:tag`.
/// Returns `None` when the reference carries no tag (including digest-only
/// refs), since a registry port's colon sits before the last `/`.
fn default_output_dir(image: &str) -> Option<std::path::PathBuf> {
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    if last_segment.contains('@') {
        return None;
    }
    let (_, tag) = last_segment.split_once(':')?;
    if tag.is_empty() {
        return None;
    }
    Some(std::path::Path::new("output").join(tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_output_dir_uses_the_image_tag() {
        assert_eq!(
            default_output_dir("ghcr.io/confidential-dot-ai/steep:base"),
            Some(PathBuf::from("output/base"))
        );
    }

    #[test]
    fn default_output_dir_ignores_a_registry_port() {
        assert_eq!(
            default_output_dir("localhost:5000/steep:v1"),
            Some(PathBuf::from("output/v1"))
        );
    }

    #[test]
    fn default_output_dir_is_none_without_a_tag() {
        assert_eq!(
            default_output_dir("ghcr.io/confidential-dot-ai/steep"),
            None
        );
        assert_eq!(default_output_dir("localhost:5000/steep"), None);
    }

    #[test]
    fn default_output_dir_is_none_for_digest_refs() {
        assert_eq!(
            default_output_dir("ghcr.io/confidential-dot-ai/steep@sha256:abc123"),
            None
        );
    }
}
