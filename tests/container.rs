use steep::container;

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

#[test]
fn test_podman_postinst_installs_podman() {
    let script = container::podman_postinst();
    assert!(script.contains("apt-get install -y podman"));
}

#[test]
fn test_podman_postinst_loads_image() {
    let script = container::podman_postinst();
    assert!(script.contains("podman load -i /opt/steep/container.oci"));
}

#[test]
fn test_podman_postinst_removes_archive() {
    let script = container::podman_postinst();
    assert!(script.contains("rm /opt/steep/container.oci"));
}

#[test]
fn test_podman_postinst_starts_with_shebang() {
    let script = container::podman_postinst();
    assert!(script.starts_with("#!/bin/bash\n"));
}
