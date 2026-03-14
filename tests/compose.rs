use steep::compose::disk;

#[test]
fn test_base_partition_conf() {
    let conf = disk::base_partition_conf(std::path::Path::new("/images/base.raw"));
    assert!(conf.contains("[Partition]"));
    assert!(conf.contains("Type=root"));
    assert!(conf.contains("CopyBlocks=/images/base.raw"));
    assert!(conf.contains("ReadOnly=yes"));
}

#[test]
fn test_project_partition_conf() {
    let conf = disk::project_partition_conf(std::path::Path::new("/images/project.raw"));
    assert!(conf.contains("[Partition]"));
    assert!(conf.contains("Type=generic"));
    assert!(conf.contains("CopyBlocks=/images/project.raw"));
    assert!(conf.contains("ReadOnly=yes"));
}
