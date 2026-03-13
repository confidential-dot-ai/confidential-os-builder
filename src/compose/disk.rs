use std::path::Path;

/// Compose a final disk image from a base partition and a project partition.
/// Creates a GPT disk image containing both partitions.
pub fn compose(
    base_partition: &Path,
    project_partition: &Path,
    output: &Path,
) -> anyhow::Result<()> {
    tracing::info!(
        base = %base_partition.display(),
        project = %project_partition.display(),
        output = %output.display(),
        "composing disk image"
    );

    if !base_partition.exists() {
        anyhow::bail!("base partition not found: {}", base_partition.display());
    }
    if !project_partition.exists() {
        anyhow::bail!("project partition not found: {}", project_partition.display());
    }

    anyhow::bail!("disk composition not yet implemented")
}
