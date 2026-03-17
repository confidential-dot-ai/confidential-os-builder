use std::path::PathBuf;

/// mkosi build profile.
#[derive(Debug, PartialEq)]
pub enum MkosiProfile {
    Base,
    CloudInit,
    Container,
    Repart,
}

/// Represents a mkosi configuration to be written as an INI file.
pub struct MkosiConfig {
    pub profile: MkosiProfile,
    pub cloud_init_dir: Option<PathBuf>,
    pub postinst_scripts: Vec<String>,
    pub extra_files: Vec<(PathBuf, Vec<u8>)>,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl MkosiConfig {
    /// Create a mkosi config for building the base partition.
    pub fn base() -> Self {
        let mut config = Self {
            profile: MkosiProfile::Base,
            cloud_init_dir: None,
            postinst_scripts: Vec::new(),
            extra_files: Vec::new(),
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![
                ("Format".to_string(), "disk".to_string()),
                ("Output".to_string(), "image.raw".to_string()),
            ],
        ));
        config
    }

    /// Create a mkosi config for building a project partition with cloud-init.
    pub fn cloud_init(cloud_init_dir: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::CloudInit,
            cloud_init_dir: Some(cloud_init_dir),
            postinst_scripts: Vec::new(),
            extra_files: Vec::new(),
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Content".to_string(),
            vec![],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![
                ("Format".to_string(), "disk".to_string()),
                ("Output".to_string(), "image.raw".to_string()),
            ],
        ));
        config
    }

    /// Create a mkosi config for building a container project partition.
    pub fn container() -> Self {
        let mut config = Self {
            profile: MkosiProfile::Container,
            cloud_init_dir: None,
            postinst_scripts: Vec::new(),
            extra_files: Vec::new(),
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Content".to_string(),
            vec![("Packages".to_string(), "podman".to_string())],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![
                ("Format".to_string(), "disk".to_string()),
                ("Output".to_string(), "image.raw".to_string()),
            ],
        ));
        config
    }

    /// Create a mkosi config for disk composition via repart.
    pub fn repart(definitions_dir: PathBuf, output: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::Repart,
            cloud_init_dir: None,
            postinst_scripts: Vec::new(),
            extra_files: Vec::new(),
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Content".to_string(),
            vec![("RepartDirectories".to_string(), definitions_dir.display().to_string())],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![
                ("Format".to_string(), "disk".to_string()),
                ("Output".to_string(), output.display().to_string()),
            ],
        ));
        config
    }

    /// Add a postinst script to be written into the mkosi build tree.
    pub fn add_postinst_script(&mut self, content: &str) {
        self.postinst_scripts.push(content.to_string());
    }

    /// Add a file to be written into the mkosi.extra/ tree.
    /// The path is relative to the image root (e.g., "etc/containers/systemd/app.container").
    pub fn add_extra_file(&mut self, relative_path: PathBuf, content: Vec<u8>) {
        self.extra_files.push((relative_path, content));
    }

    /// Write extra files to the mkosi.extra/ directory in the build tree.
    pub fn write_extra_files(&self, build_dir: &std::path::Path) -> anyhow::Result<()> {
        if self.extra_files.is_empty() {
            return Ok(());
        }
        let extra_dir = build_dir.join("mkosi.extra");
        for (relative_path, content) in &self.extra_files {
            let dest = extra_dir.join(relative_path);
            if let Some(parent) = dest.parent() {
                fs_err::create_dir_all(parent)?;
            }
            fs_err::write(&dest, content)?;
        }
        Ok(())
    }

    /// Serialize to mkosi INI format.
    pub fn to_ini(&self) -> String {
        let mut output = String::new();
        for (section, entries) in &self.sections {
            output.push_str(&format!("[{}]\n", section));
            for (key, value) in entries {
                output.push_str(&format!("{}={}\n", key, value));
            }
            output.push('\n');
        }
        output
    }

    /// Write the config to a file.
    pub fn write_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        fs_err::write(path, self.to_ini())?;
        Ok(())
    }

    /// Build the mkosi command-line arguments.
    /// The work_dir is both the config directory and the output directory.
    pub fn to_mkosi_args(&self, work_dir: &std::path::Path) -> Vec<String> {
        vec![
            "--directory".to_string(),
            work_dir.display().to_string(),
            "--output-dir".to_string(),
            work_dir.display().to_string(),
            "build".to_string(),
        ]
    }

    /// Write postinst scripts to the mkosi build tree directory.
    /// Creates mkosi.postinst.d/ with numbered scripts.
    pub fn write_postinst_scripts(&self, build_dir: &std::path::Path) -> anyhow::Result<()> {
        if self.postinst_scripts.is_empty() {
            return Ok(());
        }
        let postinst_dir = build_dir.join("mkosi.postinst.d");
        fs_err::create_dir_all(&postinst_dir)?;
        for (i, script) in self.postinst_scripts.iter().enumerate() {
            let script_path = postinst_dir.join(format!("{:02}-script.sh", i));
            fs_err::write(&script_path, script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(())
    }

    /// Invoke mkosi with the generated config.
    pub fn invoke(&self, work_dir: &std::path::Path) -> anyhow::Result<()> {
        let config_path = work_dir.join("mkosi.conf");
        self.write_to(&config_path)?;
        self.write_postinst_scripts(work_dir)?;
        self.write_extra_files(work_dir)?;
        crate::tools::require("mkosi")?;
        let args = self.to_mkosi_args(work_dir);
        tracing::info!(config = %config_path.display(), "invoking mkosi");
        crate::tools::run_command_streaming("mkosi", &args)?;
        Ok(())
    }
}
