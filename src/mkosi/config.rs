use std::path::PathBuf;

/// mkosi build profile.
#[derive(Debug, PartialEq)]
pub enum MkosiProfile {
    Base,
    CloudInit,
}

/// Represents a mkosi configuration to be written as an INI file.
pub struct MkosiConfig {
    pub profile: MkosiProfile,
    pub source_image: Option<PathBuf>,
    pub cloud_init_dir: Option<PathBuf>,
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl MkosiConfig {
    /// Create a mkosi config for building the base partition.
    pub fn base(source_image: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::Base,
            source_image: Some(source_image),
            cloud_init_dir: None,
            sections: Vec::new(),
        };
        config.sections.push((
            "Distribution".to_string(),
            vec![("Distribution".to_string(), "ubuntu".to_string())],
        ));
        config.sections.push((
            "Output".to_string(),
            vec![("Format".to_string(), "disk".to_string())],
        ));
        config
    }

    /// Create a mkosi config for building a project partition with cloud-init.
    pub fn cloud_init(cloud_init_dir: PathBuf) -> Self {
        let mut config = Self {
            profile: MkosiProfile::CloudInit,
            source_image: None,
            cloud_init_dir: Some(cloud_init_dir),
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
            vec![("Format".to_string(), "disk".to_string())],
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
}
