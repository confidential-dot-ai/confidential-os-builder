use steep::source;

#[test]
fn test_is_url_detects_https() {
    assert!(source::is_url("https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"));
}

#[test]
fn test_is_url_detects_http() {
    assert!(source::is_url("http://example.com/image.img"));
}

#[test]
fn test_is_url_rejects_local_path() {
    assert!(!source::is_url("/home/user/images/ubuntu.img"));
}

#[test]
fn test_is_url_rejects_relative_path() {
    assert!(!source::is_url("images/ubuntu.img"));
}

#[test]
fn test_filename_from_url() {
    let name = source::filename_from_url("https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img");
    assert_eq!(name, Some("noble-server-cloudimg-amd64.img".to_string()));
}

#[test]
fn test_cache_dir() {
    let dir = source::cache_dir();
    assert!(dir.ends_with("steep/base-inputs"));
}
