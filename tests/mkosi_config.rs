use std::path::PathBuf;
use steep::mkosi::config::{MkosiConfig, MkosiProfile};

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
fn test_container_config_profile() {
    let config = MkosiConfig::container();
    assert_eq!(config.profile, MkosiProfile::Container);
}

#[test]
fn test_container_config_ini() {
    let config = MkosiConfig::container();
    let ini = config.to_ini();
    assert!(ini.contains("[Distribution]"));
    assert!(ini.contains("Distribution=ubuntu"));
    assert!(ini.contains("[Content]"));
    assert!(ini.contains("Packages=podman"));
    assert!(ini.contains("[Output]"));
    assert!(ini.contains("Format=disk"));
}

#[test]
fn test_add_extra_file() {
    let mut config = MkosiConfig::container();
    config.add_extra_file(
        std::path::PathBuf::from("etc/containers/systemd/app.container"),
        b"[Container]\nImage=test\n".to_vec(),
    );
    assert_eq!(config.extra_files.len(), 1);
    assert_eq!(config.extra_files[0].0, std::path::PathBuf::from("etc/containers/systemd/app.container"));
}

#[test]
fn test_write_extra_files() {
    let mut config = MkosiConfig::container();
    config.add_extra_file(
        std::path::PathBuf::from("etc/myfile.conf"),
        b"content".to_vec(),
    );
    let dir = tempfile::tempdir().unwrap();
    config.write_extra_files(dir.path()).unwrap();
    let written = std::fs::read_to_string(dir.path().join("mkosi.extra/etc/myfile.conf")).unwrap();
    assert_eq!(written, "content");
}
