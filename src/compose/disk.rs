use std::path::Path;

use crate::mkosi::config::MkosiConfig;

/// Generate the repart partition definition for the base partition.
pub fn base_partition_conf(base_partition: &Path) -> String {
    format!(
        "[Partition]\n\
         Type=root\n\
         Format=ext4\n\
         CopyBlocks={}\n\
         ReadOnly=yes\n\
         SizeMinBytes=2G\n",
        base_partition.display()
    )
}

/// Generate the repart partition definition for the project partition.
pub fn project_partition_conf(project_partition: &Path) -> String {
    format!(
        "[Partition]\n\
         Type=generic\n\
         Format=ext4\n\
         CopyBlocks={}\n\
         ReadOnly=yes\n\
         SizeMinBytes=512M\n",
        project_partition.display()
    )
}

/// Compose a final GPT disk image from base and project partitions using mkosi repart.
pub fn compose(
    base_partition: &Path,
    project_partition: &Path,
    output: &Path,
) -> anyhow::Result<()> {
    tracing::info!(
        base = %base_partition.display(),
        project = %project_partition.display(),
        output = %output.display(),
        "composing disk image via repart"
    );

    if !base_partition.exists() {
        anyhow::bail!("base partition not found: {}", base_partition.display());
    }
    if !project_partition.exists() {
        anyhow::bail!("project partition not found: {}", project_partition.display());
    }

    let work_dir = tempfile::tempdir()?;
    let definitions_dir = work_dir.path().join("definitions");
    fs_err::create_dir_all(&definitions_dir)?;

    fs_err::write(
        definitions_dir.join("00-base.conf"),
        base_partition_conf(base_partition),
    )?;
    fs_err::write(
        definitions_dir.join("10-project.conf"),
        project_partition_conf(project_partition),
    )?;

    let config = MkosiConfig::repart(definitions_dir, output.to_path_buf());
    config.invoke(work_dir.path())?;

    Ok(())
}
