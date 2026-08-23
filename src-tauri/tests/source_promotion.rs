use std::path::PathBuf;

use rusqlite::{Connection, params};
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::core::domain::{Health, SkillId, skill_identity_key};
use skill_man_lib::seams::source_promotion_store::{
    SourcePromotionMemberOrigin, SourcePromotionMemberRecord, SourcePromotionRecord,
    SourcePromotionRemovedMemberRecord, SourcePromotionStore,
};

fn legacy_member(
    id: &str,
    directory_name: &str,
    path: &str,
    final_entity_path: &str,
) -> SourcePromotionMemberRecord {
    SourcePromotionMemberRecord {
        origin: SourcePromotionMemberOrigin::Legacy,
        skill_id: SkillId(id.into()),
        directory_name: directory_name.into(),
        identity_key: skill_identity_key(directory_name),
        display_name: directory_name.into(),
        description: String::new(),
        final_entity_path: PathBuf::from(final_entity_path),
        skill_path: path.into(),
        remote_baseline_hash: "tree-sha256-v1:target".into(),
        current_baseline_hash: "tree-sha256-v1:target".into(),
        health: Health::Healthy,
    }
}

#[test]
fn promotion_keeps_the_legacy_parent_id_and_converts_the_complete_member_set() {
    let directory = tempfile::tempdir().expect("temporary catalog");
    let catalog_path = directory.path().join("skill-man.sqlite3");
    let store = SqliteCatalogStore::open(&catalog_path).expect("open catalog");
    let connection = Connection::open(&catalog_path).expect("connection");
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
                    id, directory_name, identity_key, display_name, description,
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
    let legacy = SourcePromotionStore::read_legacy_source_promotion(&store, "remote-stable")
        .expect("read legacy facts");
    assert_eq!(legacy.remote_id, "remote-stable");
    assert_eq!(legacy.tracking_ref, "main");
    assert_eq!(legacy.members.len(), 2);

    let record = SourcePromotionRecord {
        operation_id: "source-promotion-test-1".into(),
        legacy: legacy.clone(),
        remote_id: "remote-stable".into(),
        provider: "github".into(),
        canonical_url: "https://github.com/acme/skills".into(),
        tracking_ref: "main".into(),
        release_id: "release-1".into(),
        resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        legacy_member_ids: vec![
            SkillId("legacy-alpha".into()),
            SkillId("legacy-beta".into()),
        ],
        members: vec![
            legacy_member(
                "legacy-alpha",
                "alpha",
                "skills/alpha",
                "/Library/skills/alpha",
            ),
            SourcePromotionMemberRecord {
                origin: SourcePromotionMemberOrigin::New,
                skill_id: SkillId("new-gamma".into()),
                directory_name: "gamma".into(),
                identity_key: skill_identity_key("gamma"),
                display_name: "Gamma".into(),
                description: String::new(),
                final_entity_path: PathBuf::from("/Library/skills/gamma"),
                skill_path: "skills/gamma".into(),
                remote_baseline_hash: "tree-sha256-v1:gamma".into(),
                current_baseline_hash: "tree-sha256-v1:gamma".into(),
                health: Health::Healthy,
            },
        ],
        removed_members: vec![SourcePromotionRemovedMemberRecord::Remove {
            skill_id: SkillId("legacy-beta".into()),
        }],
    };

    SourcePromotionStore::validate_source_promotion(&store, &record).expect("preflight");
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
    assert_eq!(
        (legacy_bindings, release_members, current_members),
        (0, 2, 2)
    );
    assert!(
        !beta_exists,
        "explicit Remove deletes the old member atomically"
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
    assert_eq!((promoted_sources, restored_bindings), (0, 2));
    assert!(restored_beta);
}
