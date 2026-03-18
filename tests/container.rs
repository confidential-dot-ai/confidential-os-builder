use steep::container;

// --- quadlet tests (unchanged) ---

#[test]
fn test_quadlet_contains_image() {
    let quadlet = container::quadlet("ghcr.io/org/app:latest", 8080);
    assert!(quadlet.contains("Image=ghcr.io/org/app:latest"));
}

#[test]
fn test_quadlet_contains_publish_port() {
    let quadlet = container::quadlet("ghcr.io/org/app:latest", 8080);
    assert!(quadlet.contains("PublishPort=8080:8080"));
}

#[test]
fn test_quadlet_has_restart_always() {
    let quadlet = container::quadlet("ghcr.io/org/app:latest", 443);
    assert!(quadlet.contains("Restart=always"));
}

#[test]
fn test_quadlet_has_install_section() {
    let quadlet = container::quadlet("ghcr.io/org/app:latest", 443);
    assert!(quadlet.contains("[Install]"));
    assert!(quadlet.contains("WantedBy=multi-user.target default.target"));
}

// --- user_data tests ---

#[test]
fn test_user_data_starts_with_cloud_config() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.starts_with("#cloud-config\n"));
}

#[test]
fn test_user_data_installs_podman() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.contains("- podman"));
}

#[test]
fn test_user_data_installs_nftables() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.contains("- nftables"));
}

#[test]
fn test_user_data_pulls_container() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.contains("podman pull ghcr.io/org/app:latest"));
}

#[test]
fn test_user_data_writes_nftables_rules() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.contains("tcp dport 8080 accept"));
}

#[test]
fn test_user_data_writes_quadlet() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    assert!(ud.contains("/etc/containers/systemd/app.container"));
    assert!(ud.contains("Image=ghcr.io/org/app:latest"));
}

#[test]
fn test_user_data_applies_nftables_before_pull() {
    let ud = container::user_data("ghcr.io/org/app:latest", 8080);
    let nft_pos = ud.find("nft -f").unwrap();
    let pull_pos = ud.find("podman pull").unwrap();
    assert!(nft_pos < pull_pos, "nftables must be applied before podman pull");
}

// --- meta_data tests ---

#[test]
fn test_meta_data_has_instance_id() {
    let md = container::meta_data();
    assert!(md.contains("instance-id:"));
}

#[test]
fn test_meta_data_has_hostname() {
    let md = container::meta_data();
    assert!(md.contains("local-hostname:"));
}
