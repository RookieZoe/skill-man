//! Fixture classification integration tests (spec §3.5, §10.1): the real
//! probe + FileSystem adapters against constructed fixture Homes. The exact
//! production fixture footprint — v4 Catalog tuples plus the three fixed
//! `tree-sha256-v1` hashes — must classify Pure; every deviation locks the
//! whole Home.

mod common;

use std::path::PathBuf;

use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::core::fixture_recovery::{
    FixtureClassification, FixtureEvidenceCollector, FixtureShapeMode, classify_fixture,
};

use common::{CATALOG_FILE_NAME, FixtureHome};

fn classify(home: &FixtureHome) -> FixtureClassification {
    let probe = SqliteCatalogProbe::new();
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    let collector = FixtureEvidenceCollector::new(&probe, &filesystem);
    let (db, tree) = collector
        .collect(&home.library_root, CATALOG_FILE_NAME)
        .expect("collect fixture evidence");
    classify_fixture(&db, &tree, &home.library_root, FixtureShapeMode::Legacy)
}

#[test]
fn exact_fixture_footprint_classifies_pure() {
    let home = FixtureHome::new();
    assert_eq!(classify(&home), FixtureClassification::Pure);
}

#[test]
fn modified_fixture_row_classifies_mixed() {
    let home = FixtureHome::new();
    home.with_sql("modify fixture row", |connection| {
        connection
            .execute(
                "UPDATE skills SET updated_at = '2026-08-13T10:07:02.781Z' WHERE id = 'media-xray'",
                [],
            )
            .expect("modify row");
    });
    match classify(&home) {
        FixtureClassification::Mixed { reasons } => {
            assert!(reasons.contains(&"skill_row_modified:media-xray".into()));
        }
        other => panic!("expected Mixed, got {other:?}"),
    }
}

#[test]
fn extra_file_in_fixture_tree_classifies_mixed() {
    let home = FixtureHome::new();
    std::fs::write(
        home.library_root
            .join("fixture-entities/skill-authoring/README.md"),
        "# Extra\n",
    )
    .expect("add extra file");
    match classify(&home) {
        FixtureClassification::Mixed { reasons } => {
            assert!(reasons.contains(&"tree_hash_mismatch:skill-authoring".into()));
        }
        other => panic!("expected Mixed, got {other:?}"),
    }
}

#[test]
fn legacy_audit_entity_directory_classifies_mixed() {
    let home = FixtureHome::new();
    std::fs::create_dir_all(home.library_root.join("fixture-entities/legacy-audit"))
        .expect("create legacy-audit entity");
    std::fs::write(
        home.library_root
            .join("fixture-entities/legacy-audit/SKILL.md"),
        "# Legacy audit\n",
    )
    .expect("write legacy-audit document");
    match classify(&home) {
        FixtureClassification::Mixed { reasons } => {
            assert!(reasons.contains(&"legacy_audit_entity_present".into()));
        }
        other => panic!("expected Mixed, got {other:?}"),
    }
}

#[test]
fn activation_rows_classify_mixed() {
    let home = FixtureHome::new();
    home.with_sql("seed activation row", |connection| {
        connection
            .execute(
                "INSERT INTO activations (
                    skill_id, agent_id, desired_enabled, expected_entry_path,
                    expected_target_path, observed_state
                 ) VALUES ('skill-authoring', 'claude-code', 1, '/tmp/entry', '/tmp/target', 'missing')",
                [],
            )
            .expect("seed activation");
    });
    match classify(&home) {
        FixtureClassification::Mixed { reasons } => {
            assert!(reasons.contains(&"activation_rows:1".into()));
        }
        other => panic!("expected Mixed, got {other:?}"),
    }
}

#[test]
fn unreadable_catalog_classifies_unknown() {
    let home = FixtureHome::new();
    std::fs::write(home.catalog_path(), b"not a sqlite database at all")
        .expect("corrupt the Catalog");
    let probe = SqliteCatalogProbe::new();
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    let collector = FixtureEvidenceCollector::new(&probe, &filesystem);
    let result = collector.collect(&home.library_root, CATALOG_FILE_NAME);
    match result {
        Err(_) => {}
        Ok((db, tree)) => {
            match classify_fixture(&db, &tree, &home.library_root, FixtureShapeMode::Legacy) {
                FixtureClassification::Unknown { .. } | FixtureClassification::Mixed { .. } => {}
                other => panic!("expected unknown/mixed for corrupt Catalog, got {other:?}"),
            }
        }
    }
}

#[test]
fn missing_catalog_but_present_trees_classifies_mixed() {
    let home = FixtureHome::new();
    std::fs::remove_file(home.catalog_path()).expect("remove Catalog");
    std::fs::remove_file(PathBuf::from(format!(
        "{}-wal",
        home.catalog_path().display()
    )))
    .ok();
    std::fs::remove_file(PathBuf::from(format!(
        "{}-shm",
        home.catalog_path().display()
    )))
    .ok();
    match classify(&home) {
        FixtureClassification::Mixed { reasons } => {
            assert!(reasons.contains(&"missing_table:skills".into()));
        }
        other => panic!("expected Mixed, got {other:?}"),
    }
}

#[test]
fn clean_bound_home_classifies_clean() {
    let home = common::BoundTestHome::new();
    let probe = SqliteCatalogProbe::new();
    let filesystem = MacOsFileSystem::new(home.dir.path().to_path_buf());
    let collector = FixtureEvidenceCollector::new(&probe, &filesystem);
    let (db, tree) = collector
        .collect(&home.library_root, CATALOG_FILE_NAME)
        .expect("collect clean evidence");
    assert_eq!(
        classify_fixture(&db, &tree, &home.library_root, FixtureShapeMode::Bound),
        FixtureClassification::Clean
    );
}
