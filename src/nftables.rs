/// Generate nftables rules for the project partition.
/// Opens a single TCP port for inbound traffic.
/// Outbound traffic is allowed (policy accept).
pub fn service_rules(port: u16) -> String {
    format!(
        "#!/usr/sbin/nft -f\nflush ruleset\ntable inet filter {{\n    chain input {{\n        type filter hook input priority 0; policy drop;\n        iif \"lo\" accept\n        ct state established,related accept\n        tcp dport {port} accept\n    }}\n    chain forward {{\n        type filter hook forward priority 0; policy drop;\n    }}\n    chain output {{\n        type filter hook output priority 0; policy accept;\n    }}\n}}\n"
    )
}
