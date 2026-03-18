use crate::nftables;

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

/// Generate cloud-init user-data that sets up the container workload.
///
/// Installs podman and nftables, writes firewall rules and a quadlet unit,
/// then pulls the container image and starts the service.
pub fn user_data(url: &str, service_port: u16) -> String {
    let nft_rules = nftables::service_rules(service_port);
    let quadlet_content = quadlet(url, service_port);

    let mut s = String::new();
    s.push_str("#cloud-config\n");
    s.push_str("packages:\n");
    s.push_str("  - podman\n");
    s.push_str("  - nftables\n");
    s.push_str("\n");
    s.push_str("write_files:\n");
    s.push_str("  - path: /etc/nftables.conf\n");
    s.push_str("    content: |\n");
    for line in nft_rules.lines() {
        s.push_str(&format!("      {line}\n"));
    }
    s.push_str("  - path: /etc/containers/systemd/app.container\n");
    s.push_str("    content: |\n");
    for line in quadlet_content.lines() {
        s.push_str(&format!("      {line}\n"));
    }
    s.push_str("\n");
    s.push_str("runcmd:\n");
    s.push_str("  - nft -f /etc/nftables.conf\n");
    s.push_str(&format!("  - podman pull {url}\n"));
    s.push_str("  - systemctl daemon-reload\n");
    s.push_str("  - systemctl start app\n");
    s
}

/// Generate cloud-init meta-data for a container workload.
pub fn meta_data() -> String {
    "instance-id: steep-container\nlocal-hostname: steep\n".to_string()
}
