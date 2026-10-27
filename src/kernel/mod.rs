pub mod aml;
pub mod compile;
pub mod config;
pub mod fetch;
pub mod ipe;
pub mod manifest;
pub mod snp_mr;
pub mod version;

/// Every kernel patch. Each names the kernel it was audited on, and
/// `kernel/version` must match all of them.
pub const PATCHES: [&str; 2] = [aml::PATCH, snp_mr::PATCH];

#[cfg(test)]
mod tests {
    #[test]
    fn patches_lists_every_committed_patch() {
        let mut committed: Vec<String> = fs_err::read_dir("kernel/patches")
            .unwrap()
            .map(|e| e.unwrap().path().to_string_lossy().into_owned())
            .filter(|p| p.ends_with(".patch"))
            .collect();
        committed.sort();
        assert_eq!(committed, super::PATCHES);
    }
}
