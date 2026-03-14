use std::path::PathBuf;
use steep::mkosi::config::{MkosiConfig, MkosiProfile};

#[test]
fn test_base_config_generates_valid_ini() {
    let config = MkosiConfig::base(PathBuf::from("/path/to/ubuntu.img"));
    let ini = config.to_ini();
    assert!(ini.contains("[Distribution]"));
    assert!(ini.contains("Distribution=ubuntu"));
}

#[test]
fn test_cloud_init_config_includes_cloud_init_dir() {
    let config = MkosiConfig::cloud_init(PathBuf::from("/path/to/cloud-init"));
    let ini = config.to_ini();
    assert!(ini.contains("[Content]"));
}

#[test]
fn test_config_profile() {
    let config = MkosiConfig::base(PathBuf::from("/path/to/img"));
    assert_eq!(config.profile, MkosiProfile::Base);
}

#[test]
fn test_add_postinst_script() {
    let mut config = MkosiConfig::base(PathBuf::from("/path/to/img"));
    config.add_postinst_script("#!/bin/bash\necho hello");
    assert_eq!(config.postinst_scripts.len(), 1);
    assert!(config.postinst_scripts[0].contains("echo hello"));
}

#[test]
fn test_repart_config() {
    let config = MkosiConfig::repart(
        PathBuf::from("/path/to/definitions"),
        PathBuf::from("/path/to/output.raw"),
    );
    assert_eq!(config.profile, MkosiProfile::Repart);
    let ini = config.to_ini();
    assert!(ini.contains("[Output]"));
}

#[test]
fn test_invoke_args_base() {
    let config = MkosiConfig::base(PathBuf::from("/path/to/img"));
    let args = config.to_mkosi_args(std::path::Path::new("/work"));
    assert!(args.contains(&"build".to_string()));
    assert!(args.contains(&"--directory".to_string()));
}
