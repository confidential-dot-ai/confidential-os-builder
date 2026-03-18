use std::path::PathBuf;

/// mkosi build profile.
#[derive(Debug, PartialEq)]
pub enum MkosiProfile {
    Repart,
}

/// Represents a mkosi configuration to be written as an INI file.
pub struct MkosiConfig {
    pub profile: MkosiProfile,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl MkosiConfig {
    /// Create a mkosi config for disk composition via repart.
    pub fn repart(definitions_dir: PathBuf, output: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::Repart,
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
    pub fn to_mkosi_args(&self, work_dir: &std::path::Path) -> Vec<String> {
        vec![
            "--directory".to_string(),
            work_dir.display().to_string(),
            "--output-dir".to_string(),
            work_dir.display().to_string(),
            "build".to_string(),
        ]
    }

    /// Invoke mkosi with the generated config.
    pub fn invoke(&self, work_dir: &std::path::Path) -> anyhow::Result<()> {
        let config_path = work_dir.join("mkosi.conf");
        self.write_to(&config_path)?;
        crate::tools::require("mkosi")?;
        let args = self.to_mkosi_args(work_dir);
        tracing::info!(config = %config_path.display(), "invoking mkosi");
        crate::tools::run_command_streaming("mkosi", &args)?;
        Ok(())
    }
}
