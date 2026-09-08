use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::seams::filesystem::FileSystem;

#[test]
fn scan_excludes_dependency_subtrees_but_preserves_full_transfer_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("SKILL.md"), "# Local\n").unwrap();
    let fs = MacOsFileSystem::new(root.to_path_buf());
    let baseline = fs.scan_tree_statistics(root, &mut |_| Ok(true)).unwrap();
    let full = fs.tree_hash(root).unwrap();
    for name in [".venv", "venv", "node_modules"] {
        let dependency = root.join(name).join("nested");
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(dependency.join("SKILL.md"), "dependency, not a skill").unwrap();
    }
    let mut paths = Vec::new();
    let observed = fs
        .scan_tree_statistics(root, &mut |entry| {
            paths.push(entry.relative_path.clone());
            Ok(true)
        })
        .unwrap();
    assert_eq!(observed.tree_hash, baseline.tree_hash);
    assert_eq!(
        observed.tree_hash,
        Some(fs.skill_observation_snapshot(root).unwrap().content_hash)
    );
    assert_eq!(observed.file_count, 1);
    assert_eq!(paths, [std::path::PathBuf::from("SKILL.md")]);
    assert_ne!(fs.tree_hash(root).unwrap(), full);
    std::fs::write(root.join("SKILL.md"), "# Changed\n").unwrap();
    assert_ne!(
        fs.scan_tree_statistics(root, &mut |_| Ok(true))
            .unwrap()
            .tree_hash,
        baseline.tree_hash
    );
}

#[test]
fn observation_prunes_nested_dependencies_but_not_similar_names_or_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/venv"), "ordinary file").unwrap();
    std::fs::create_dir_all(root.join("node_modules-not-dependencies")).unwrap();
    std::fs::write(
        root.join("node_modules-not-dependencies/code.js"),
        "real code",
    )
    .unwrap();
    let fs = MacOsFileSystem::new(root.to_path_buf());
    let before = fs.skill_observation_snapshot(root).unwrap();
    std::fs::create_dir_all(root.join("scripts/node_modules/pkg/.git")).unwrap();
    std::fs::write(
        root.join("scripts/node_modules/pkg/.git/config"),
        "dependency origin",
    )
    .unwrap();
    std::os::unix::fs::symlink("/not/a/real/directory", root.join(".venv")).unwrap();
    let scan = fs.scan_tree_statistics(root, &mut |_| Ok(true)).unwrap();
    assert_eq!(scan.file_count, 2);
    assert_eq!(scan.tree_hash, Some(before.content_hash.clone()));
    assert_eq!(
        fs.skill_observation_snapshot(root).unwrap().content_hash,
        before.content_hash
    );
    std::fs::write(root.join("scripts/venv"), "changed ordinary file").unwrap();
    assert_ne!(
        fs.skill_observation_snapshot(root).unwrap().content_hash,
        before.content_hash
    );
}

#[test]
fn streaming_hash_uses_global_path_byte_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("package")).unwrap();
    std::fs::create_dir_all(root.join("package-1.dist-info")).unwrap();
    std::fs::write(root.join("package/init.py"), "code").unwrap();
    let fs = MacOsFileSystem::new(root.to_path_buf());
    assert_eq!(
        fs.scan_tree_statistics(root, &mut |_| Ok(true))
            .unwrap()
            .tree_hash,
        Some(fs.tree_hash(root).unwrap())
    );
}
