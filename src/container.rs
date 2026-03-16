use std::path::Path;

use crate::tools;

/// Pull an OCI container image using podman.
/// If the image already exists in the local store, the pull is skipped.
pub fn pull(url: &str) -> anyhow::Result<()> {
    let exists = std::process::Command::new("podman")
        .args(["image", "exists", url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if exists {
        tracing::info!(url = url, "container image already present locally, skipping pull");
        return Ok(());
    }
    tracing::info!(url = url, "pulling container image");
    tools::run_command_streaming("podman", &["pull", url])?;
    Ok(())
}

/// Save a container image to an OCI archive.
pub fn save(url: &str, dest: &Path) -> anyhow::Result<()> {
    let dest_str = dest.display().to_string();
    tracing::info!(url = url, dest = %dest_str, "saving container image to archive");
    tools::run_command_streaming("podman", &["save", "-o", &dest_str, url])?;
    Ok(())
}

/// Generate a podman quadlet .container unit file.
pub fn quadlet(url: &str, service_port: u16) -> String {
    format!(
        "[Container]\n\
         Image={url}\n\
         PublishPort={service_port}:{service_port}\n\
         \n\
         [Service]\n\
         Restart=always\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target default.target\n"
    )
}

/// Generate the postinst script that installs podman and loads the baked OCI image.
pub fn podman_postinst() -> String {
    "#!/bin/bash\n\
     set -euo pipefail\n\
     apt-get install -y podman\n\
     podman load -i /opt/steep/container.oci\n\
     rm /opt/steep/container.oci\n"
        .to_string()
}
