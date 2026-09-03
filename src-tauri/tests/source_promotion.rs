use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::domain::skill_identity_key;
use skill_man_lib::seams::source_promotion_store::{
    LegacySourcePromotionMemberRecord, SourcePromotionMemberOrigin, SourcePromotionMemberRecord,
    SourcePromotionRecord, SourcePromotionRemovedMemberRecord, SourcePromotionStore,
};

fn legacy_member(
    id: &str,
    directory_name: &str,
    path: &str,
    final_entity_path: &str,
) -> LegacySourcePromotionMemberRecord {
    LegacySourcePromotionMemberRecord {
        skill_id: skill_man_lib::core::domain::SkillId(id.into()),
        directory_name: directory_name.into(),
        skill_path: path.into(),
        current_baseline_hash: "tree-sha256-v1:legacy".into(),
        final_entity_path: PathBuf::from(final_entity_path),
        identity_key: skill_identity_key(directory_name),
        display_name: directory_name.into(),
        description: String::new(),
        library_entry_path: PathBuf::from(final_entity_path),
        recorded_content_hash: "tree-sha256-v1:legacy".into(),
        health: skill_man_lib::core::domain::Health::Healthy,
        requested_ref: "main".into(),
        verification_anchor_commit: "old-anchor".into(),
        original_commit_known: true,
        provider_hash: None,
        remote_baseline_hash: "tree-sha256-v1:legacy".into(),
        last_checked_at: None,
        last_updated_at: None,
        activations: Vec::new(),
    }
}

fn promote_fixture(catalog_path: &std::path::Path) {
    let connection = Connection::open(catalog_path).expect("connection");
    connection
        .execute(
            "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
             VALUES ('remote-stable', 'https://github.com/acme/skills', 'then')",
            [],
        )
        .expect("legacy parent");
    for (id, name, path) in [
        ("legacy-alpha", "alpha", "skills/alpha"),
        ("legacy-beta", "beta", "skills/beta"),
    ] {
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?2, '', 'remote_install', ?4, ?4,
                    'tree-sha256-v1:legacy', 'healthy', 'now', 'now')",
                params![
                    id,
                    name,
                    skill_identity_key(name),
                    format!("/Library/skills/{name}")
                ],
            )
            .expect("legacy skill");
        connection
            .execute(
                "INSERT INTO remote_bindings (
                    skill_id, remote_id, requested_ref, verification_anchor_commit,
                    original_commit_known, skill_path, provider_hash,
                    remote_baseline_hash, current_baseline_hash
                 ) VALUES (?1, 'remote-stable', 'main', 'old-anchor', 1, ?2, NULL,
                    'tree-sha256-v1:legacy', 'tree-sha256-v1:legacy')",
                params![id, path],
            )
            .expect("legacy binding");
    }
}

#[test]
fn promotion_keeps_the_legacy_parent_id_and_converts_the_complete_member_set() {
    let directory = tempfile::tempdir().expect("temporary catalog");
    let catalog_path = directory.path().join("skill-man.sqlite3");
    let store = SqliteCatalogStore::open(&catalog_path).expect("open catalog");
    promote_fixture(&catalog_path);
    let legacy = SourcePromotionStore::read_legacy_source_promotion(&store, "remote-stable")
        .expect("read legacy facts");
    assert_eq!(legacy.remote_id, "remote-stable");
    assert_eq!(legacy.tracking_ref, "main");
    assert_eq!(legacy.members.len(), 2);

    let alpha = legacy_member(
        "legacy-alpha",
        "alpha",
        "skills/alpha",
        "/Library/skills/alpha",
    );
    let record = SourcePromotionRecord {
        operation_id: "source-promotion-test-1".into(),
        legacy: legacy.clone(),
        remote_id: "remote-stable".into(),
        provider: "github".into(),
        canonical_url: "https://github.com/acme/skills".into(),
        tracking_mode: "branch".into(),
        tracking_value: Some("main".into()),
        selection_kind: "branch".into(),
        selected_ref: "main".into(),
        release_id: "release-1".into(),
        resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        legacy_member_ids: vec![
            skill_man_lib::core::domain::SkillId("legacy-alpha".into()),
            skill_man_lib::core::domain::SkillId("legacy-beta".into()),
        ],
        members: vec![
            SourcePromotionMemberRecord {
                origin: SourcePromotionMemberOrigin::Legacy,
                skill_id: skill_man_lib::core::domain::SkillId("legacy-alpha".into()),
                directory_name: "alpha".into(),
                identity_key: skill_identity_key("alpha"),
                display_name: "Alpha".into(),
                description: String::new(),
                storage_relpath: "skills/git/remote-stable/legacy-alpha".into(),
                skill_path: "skills/alpha".into(),
                tree_hash: "tree-sha256-v1:target".into(),
                provider_hash: None,
                legacy_entity: Some(alpha.clone()),
            },
            SourcePromotionMemberRecord {
                origin: SourcePromotionMemberOrigin::New,
                skill_id: skill_man_lib::core::domain::SkillId("new-gamma".into()),
                directory_name: "gamma".into(),
                identity_key: skill_identity_key("gamma"),
                display_name: "Gamma".into(),
                description: String::new(),
                storage_relpath: "skills/git/remote-stable/new-gamma".into(),
                skill_path: "skills/gamma".into(),
                tree_hash: "tree-sha256-v1:gamma".into(),
                provider_hash: None,
                legacy_entity: None,
            },
        ],
        removed_members: vec![SourcePromotionRemovedMemberRecord {
            skill_id: skill_man_lib::core::domain::SkillId("legacy-beta".into()),
            directory_name: "beta".into(),
            legacy_entity: legacy_member(
                "legacy-beta",
                "beta",
                "skills/beta",
                "/Library/skills/beta",
            ),
        }],
    };

    SourcePromotionStore::validate_source_promotion(&store, &record).expect("preflight");
    let connection = Connection::open(&catalog_path).expect("connection");
    connection
        .execute(
            "UPDATE remote_bindings SET verification_anchor_commit = 'changed-anchor'
             WHERE skill_id = 'legacy-alpha'",
            [],
        )
        .expect("change frozen Legacy fact");
    assert!(
        SourcePromotionStore::validate_source_promotion(&store, &record).is_err(),
        "the final preflight must reject a changed Legacy binding fact"
    );
    connection
        .execute(
            "UPDATE remote_bindings SET verification_anchor_commit = 'old-anchor'
             WHERE skill_id = 'legacy-alpha'",
            [],
        )
        .expect("restore frozen Legacy fact");
    SourcePromotionStore::commit_source_promotion(&store, record.clone())
        .expect("commit promotion");
    assert!(
        SourcePromotionStore::source_promotion_is_committed(&store, &record)
            .expect("whole release is current")
    );

    let remote_id: String = connection
        .query_row("SELECT remote_id FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("preserved parent id");
    assert_eq!(remote_id, "remote-stable");
    let (tracking_mode, tracking_value, selected_ref): (String, Option<String>, String) =
        connection
            .query_row(
                "SELECT tracking_mode, tracking_value, current_selected_ref
             FROM git_repository_sources",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("v9 tracking facts");
    assert_eq!(tracking_mode, "branch");
    assert_eq!(tracking_value.as_deref(), Some("main"));
    assert_eq!(selected_ref, "main");
    let selection_kind: String = connection
        .query_row(
            "SELECT selection_kind FROM git_source_releases",
            [],
            |row| row.get(0),
        )
        .expect("v9 selection kind");
    assert_eq!(selection_kind, "branch");
    let legacy_bindings: i64 = connection
        .query_row("SELECT COUNT(*) FROM remote_bindings", [], |row| row.get(0))
        .expect("legacy bindings removed");
    let release_members: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_release_members",
            [],
            |row| row.get(0),
        )
        .expect("whole discovered release");
    let current_members: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_members", [], |row| {
            row.get(0)
        })
        .expect("whole current member set");
    let beta_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE id = 'legacy-beta')",
            [],
            |row| row.get(0),
        )
        .expect("removed member state");
    let alpha_path: (String, String) = connection
        .query_row(
            "SELECT final_entity_path, recorded_content_hash FROM skills WHERE id = 'legacy-alpha'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("converted member");
    assert_eq!(alpha_path.0, "skills/git/remote-stable/legacy-alpha");
    assert_eq!(alpha_path.1, "tree-sha256-v1:target");
    let alpha_binding_still_audit: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_bindings WHERE skill_id = 'legacy-alpha')",
            [],
            |row| row.get(0),
        )
        .expect("audit facts");
    assert!(
        !alpha_binding_still_audit,
        "legacy bindings remain audit-only in the frozen promotion journal"
    );
    assert_eq!(
        (legacy_bindings, release_members, current_members),
        (0, 2, 2)
    );
    assert!(
        !beta_exists,
        "an absent legacy member is removed atomically"
    );

    SourcePromotionStore::undo_source_promotion(&store, &record, &legacy)
        .expect("undo restores the frozen whole Legacy source");
    let promoted_sources: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("Git Repository Source removed on undo");
    let restored_bindings: i64 = connection
        .query_row("SELECT COUNT(*) FROM remote_bindings", [], |row| row.get(0))
        .expect("frozen Legacy audit facts restored");
    let restored_beta: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE id = 'legacy-beta')",
            [],
            |row| row.get(0),
        )
        .expect("removed legacy member restored");
    let alpha_restored: (String, String) = connection
        .query_row(
            "SELECT final_entity_path, recorded_content_hash FROM skills WHERE id = 'legacy-alpha'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("restored member");
    assert_eq!(
        alpha_restored.0, "/Library/skills/alpha",
        "Legacy current members return to their pre-promotion entities"
    );
    assert_eq!(alpha_restored.1, "tree-sha256-v1:legacy");
    let gamma_gone: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM skills WHERE id = 'new-gamma')",
            [],
            |row| row.get(0),
        )
        .expect("new member removed");
    assert!(!gamma_gone);
    assert_eq!((promoted_sources, restored_bindings), (0, 2));
    assert!(restored_beta);
}

#[test]
fn promotion_refuses_a_partial_or_ambiguous_legacy_parent() {
    let directory = tempfile::tempdir().expect("temporary catalog");
    let catalog_path = directory.path().join("skill-man.sqlite3");
    let store = SqliteCatalogStore::open(&catalog_path).expect("open catalog");
    promote_fixture(&catalog_path);
    assert!(SourcePromotionStore::read_legacy_source_promotion(&store, "remote-missing").is_err());
    let unknown: Option<()> = Connection::open(&catalog_path)
        .expect("connection")
        .query_row(
            "SELECT 1 FROM remote_source_parents WHERE remote_id = 'remote-missing'",
            [],
            |_| Ok(()),
        )
        .optional()
        .expect("read");
    assert!(unknown.is_none());
}
