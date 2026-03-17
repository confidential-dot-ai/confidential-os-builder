use steep::nftables;

#[test]
fn test_service_rules_opens_port() {
    let rules = nftables::service_rules(8080);
    assert!(rules.contains("tcp dport 8080 accept"));
}

#[test]
fn test_service_rules_output_policy_is_accept() {
    let rules = nftables::service_rules(443);
    assert!(rules.contains("chain output {\n        type filter hook output priority 0; policy accept;"));
}

#[test]
fn test_service_rules_starts_with_shebang() {
    let rules = nftables::service_rules(443);
    assert!(rules.starts_with("#!/usr/sbin/nft -f\n"));
}
