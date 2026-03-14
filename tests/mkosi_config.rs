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
