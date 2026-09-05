//! Store-level Source lifecycle tests (ticket #93, spec §8.3–§8.4,
//! ADR-0018): complete source facts, update commit/undo, snapshot-mismatch
//! health, Create Local Source Copy registration and whole-source Remove.
//! The rows are seeded through real SQL and every assertion reads the
//! SQLite file through the same RuntimeCatalogStore production uses.

mod common;

use std::path::PathBuf;

use rusqlite::Connection;
use skill_man_lib::core::domain::{Health, SkillId, skill_identity_key};
use skill_man_lib::seams::source_transition_store::{
    SourceTransitionMemberRecord, SourceTransitionRecord, SourceTransitionStore,
};
use skill_man_lib::seams::source_update_store::{
    LocalSourceCopyRecord, SourceMemberPresence, SourceUpdateMemberOrigin,
    SourceUpdateMemberRecord, SourceUpdatePreviousMemberRecord, SourceUpdateRecord,
    SourceUpdateRemovedMemberRecord, SourceUpdateStore, SourceUpdateStoreError,
};

const REMOTE_ID: &str = "remote-source-1";
const RELEASE_R1: &str = "release-r1";
const COMMIT_R1: &str = "1111111111111111111111111111111111111111";

fn seed_managed_source(home: &common::BoundTestHome) {
    home.with_sql("seed managed source", |connection| {
        let batches = [
            (
                "parents + source",
                &format!(
                    r#"
                    INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                    VALUES ('{REMOTE_ID}', 'https://github.com/acme/skills', '2026-08-01T00:00:00Z');
                    INSERT INTO git_repository_sources (
                        remote_id, provider, canonical_url, tracking_mode, tracking_value,
                        current_selected_ref, current_release_id, created_at, updated_at
                    ) VALUES (
                        '{REMOTE_ID}', 'github', 'https://github.com/acme/skills',
                        'auto_release_tag_head', NULL, 'v1.0.0', NULL,
                        '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z'
                    );
                    "#,
                ),
            ),
            (
                "releases",
                &format!(
                    r#"
                    INSERT INTO git_source_releases (
                        release_id, remote_id, selection_kind, selected_ref, resolved_commit, discovered_at
                    ) VALUES (
                        '{RELEASE_R1}', '{REMOTE_ID}', 'tag', 'v1.0.0', '{COMMIT_R1}', '2026-08-01T00:00:00Z'
                    );
                    UPDATE git_repository_sources
                       SET current_release_id = '{RELEASE_R1}'
                     WHERE remote_id = '{REMOTE_ID}';
                    "#,
                ),
            ),
            (
                "skills",
                &format!(
                    r#"
                    INSERT INTO skills (
                        id, directory_name, directory_identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                    ) VALUES
                        ('skill-a', 'alpha', '{}', 'Alpha', 'First.', 'remote_install',
                         'skills/git/{REMOTE_ID}/skill-a', 'skills/git/{REMOTE_ID}/skill-a',
                         'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'healthy',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z'),
                        ('skill-b', 'beta', '{}', 'Beta', 'Second.', 'remote_install',
                         'skills/git/{REMOTE_ID}/skill-b', 'skills/git/{REMOTE_ID}/skill-b',
                         'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'healthy',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                    "#,
                    skill_identity_key("alpha"),
                    skill_identity_key("beta"),
                ),
            ),
            (
                "release members",
                &format!(
                    r#"
                    INSERT INTO git_source_release_members (
                        release_id, skill_id, skill_path, directory_name,
                        directory_identity_key, tree_hash, provider_hash
                    ) VALUES
                        ('{RELEASE_R1}', 'skill-a', 'skills/alpha', 'alpha', '{}',     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'pa'),
                        ('{RELEASE_R1}', 'skill-b', 'skills/beta',  'beta', '{}',  'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', NULL);
                    "#,
                    skill_identity_key("alpha"),
                    skill_identity_key("beta"),
                ),
            ),
            (
                "members",
                &format!(
                    r#"
                    INSERT INTO git_source_members (
                        skill_id, remote_id, skill_path, storage_relpath, presence,
                        first_seen_release_id, last_seen_release_id, last_checked_at, last_updated_at
                    ) VALUES
                        ('skill-a', '{REMOTE_ID}', 'skills/alpha',
                         'skills/git/{REMOTE_ID}/skill-a', 'current',
                         '{RELEASE_R1}', '{RELEASE_R1}', unixepoch('now'), unixepoch('now')),
                        ('skill-b', '{REMOTE_ID}', 'skills/beta',
                         'skills/git/{REMOTE_ID}/skill-b', 'current',
                         '{RELEASE_R1}', '{RELEASE_R1}', unixepoch('now'), unixepoch('now'));
                    "#,
                ),
            ),
        ];
        for (label, sql) in batches {
            connection.execute_batch(sql).unwrap_or_else(|error| {
                panic!("{label} seed failed: {error}");
            });
        }
    });
}

fn previous_from_current(
    current: &skill_man_lib::seams::source_update_store::SourceUpdateCurrentSource,
) -> Vec<SourceUpdatePreviousMemberRecord> {
    current
        .members
        .iter()
        .map(|member| SourceUpdatePreviousMemberRecord {
            skill_id: member.skill_id.clone(),
            directory_name: member.directory_name.clone(),
            identity_key: member.identity_key.clone(),
            display_name: member.display_name.clone(),
            description: member.description.clone(),
            skill_path: member.skill_path.clone(),
            storage_relpath: member.storage_relpath.clone(),
            presence: member.presence,
            tree_hash: member.tree_hash.clone(),
            health: member.health,
        })
        .collect()
}

fn target_member(
    origin: SourceUpdateMemberOrigin,
    skill_id: &str,
    skill_path: &str,
    directory_name: &str,
    tree_hash: &str,
) -> SourceUpdateMemberRecord {
    SourceUpdateMemberRecord {
        origin,
        skill_id: SkillId(skill_id.into()),
        directory_name: directory_name.into(),
        identity_key: skill_identity_key(directory_name),
        display_name: directory_name.into(),
        description: String::new(),
        storage_relpath: format!("skills/git/{REMOTE_ID}/{skill_id}"),
        skill_path: skill_path.into(),
        tree_hash: tree_hash.into(),
        provider_hash: None,
    }
}

#[test]
fn read_current_exposes_complete_source_facts_with_tombstone() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read current")
        .expect("source");
    assert_eq!(current.remote_id, REMOTE_ID);
    assert_eq!(current.selected_ref, "v1.0.0");
    assert_eq!(current.resolved_commit, COMMIT_R1);
    assert_eq!(current.current_release_id, RELEASE_R1);
    assert_eq!(current.tracking_mode, "auto_release_tag_head");
    assert_eq!(current.members.len(), 2);
    let alpha = current
        .members
        .iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha member");
    assert_eq!(alpha.presence, SourceMemberPresence::Current);
    assert_eq!(
        alpha.tree_hash.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(alpha.storage_relpath, "skills/git/remote-source-1/skill-a");
    assert_eq!(alpha.health, Health::Healthy);
    // Unknown source: typed None, not a guess.
    assert!(
        home.runtime
            .read_current("missing")
            .expect("read missing")
            .is_none()
    );
}

#[test]
fn source_update_commit_publishes_target_release_and_flips_presence() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let record = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-1".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_from_current(&current),
        // alpha stays; beta removed; delta added.
        members: vec![
            target_member(
                SourceUpdateMemberOrigin::Existing,
                "skill-a",
                "skills/alpha",
                "alpha",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            target_member(
                SourceUpdateMemberOrigin::New,
                "skill-d",
                "skills/delta",
                "delta",
                "dddddddddddddddddddddddddddddddddddddddd",
            ),
        ],
        removed_members: vec![SourceUpdateRemovedMemberRecord {
            skill_id: SkillId("skill-b".into()),
            directory_name: "beta".into(),
            skill_path: "skills/beta".into(),
            storage_relpath: "skills/git/remote-source-1/skill-b".into(),
            previous_tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        }],
    };
    home.runtime
        .validate_source_update(&record)
        .expect("validate update");
    let version = home
        .runtime
        .commit_source_update(&record)
        .expect("commit update");
    assert!(version > 0);

    let after = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    assert_eq!(after.selected_ref, "v1.1.0");
    assert_eq!(after.current_release_id, "release-r2");
    assert_eq!(
        after.resolved_commit,
        "2222222222222222222222222222222222222222"
    );
    let alpha = after
        .members
        .iter()
        .find(|member| member.skill_path == "skills/alpha")
        .expect("alpha");
    assert_eq!(alpha.skill_id.0, "skill-a");
    assert_eq!(alpha.presence, SourceMemberPresence::Current);
    assert_eq!(alpha.health, Health::Healthy);
    let beta = after
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta tombstone");
    assert_eq!(beta.presence, SourceMemberPresence::Absent);
    assert_eq!(beta.health, Health::Broken);
    assert_eq!(beta.last_seen_release_id, "release-r2");
    let delta = after
        .members
        .iter()
        .find(|member| member.skill_path == "skills/delta")
        .expect("delta");
    assert_eq!(delta.skill_id.0, "skill-d");
    assert_eq!(delta.presence, SourceMemberPresence::Current);
    // New member is present in the Library but has no Activation rows yet
    // (default not enabled).
    let enabled: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row(
            "SELECT COUNT(*) FROM activations WHERE skill_id = 'skill-d'",
            [],
            |row| row.get(0),
        )
        .expect("activation count");
    assert_eq!(enabled, 0);
    assert!(
        home.runtime
            .source_update_is_committed(&record)
            .expect("committed probe")
    );
}

#[test]
fn source_update_undo_restores_exact_previous_state() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let previous_members = previous_from_current(&current);
    let record = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-2".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_members.clone(),
        members: vec![
            target_member(
                SourceUpdateMemberOrigin::Existing,
                "skill-a",
                "skills/alpha",
                "alpha",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            target_member(
                SourceUpdateMemberOrigin::New,
                "skill-e",
                "skills/echo",
                "echo",
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            ),
        ],
        removed_members: vec![SourceUpdateRemovedMemberRecord {
            skill_id: SkillId("skill-b".into()),
            directory_name: "beta".into(),
            skill_path: "skills/beta".into(),
            storage_relpath: "skills/git/remote-source-1/skill-b".into(),
            previous_tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        }],
    };
    home.runtime
        .commit_source_update(&record)
        .expect("commit update");
    home.runtime
        .undo_source_update(&record)
        .expect("undo update");

    let restored = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    assert_eq!(restored.selected_ref, "v1.0.0");
    assert_eq!(restored.current_release_id, RELEASE_R1);
    let beta = restored
        .members
        .iter()
        .find(|member| member.skill_path == "skills/beta")
        .expect("beta restored");
    assert_eq!(beta.presence, SourceMemberPresence::Current);
    assert_eq!(beta.health, Health::Healthy);
    assert!(
        restored
            .members
            .iter()
            .all(|member| member.skill_path != "skills/echo")
    );
    // The new release rows are gone with their member registry.
    let releases: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row(
            "SELECT COUNT(*) FROM git_source_releases WHERE release_id = 'release-r2'",
            [],
            |row| row.get(0),
        )
        .expect("release count");
    assert_eq!(releases, 0);
    assert!(
        !home
            .runtime
            .source_update_is_committed(&record)
            .expect("committed probe")
    );
}

#[test]
fn source_update_keeps_preexisting_tombstones_in_commit_and_undo() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    home.with_sql("seed preexisting tombstone", |connection| {
        connection
            .execute_batch(
                r#"
                UPDATE git_source_members
                   SET presence = 'absent',
                       last_seen_release_id = 'release-r1'
                 WHERE skill_id = 'skill-b';
                UPDATE skills
                   SET health = 'broken'
                 WHERE id = 'skill-b';
                "#,
            )
            .expect("seed tombstone");
    });
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let record = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-existing-tombstone".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_from_current(&current),
        members: vec![target_member(
            SourceUpdateMemberOrigin::Existing,
            "skill-a",
            "skills/alpha",
            "alpha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )],
        removed_members: Vec::new(),
    };

    home.runtime
        .commit_source_update(&record)
        .expect("commit update with existing tombstone");
    let after = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read after")
        .expect("source after");
    let tombstone = after
        .members
        .iter()
        .find(|member| member.skill_id.0 == "skill-b")
        .expect("preexisting tombstone");
    assert_eq!(tombstone.presence, SourceMemberPresence::Absent);
    assert_eq!(tombstone.health, Health::Broken);
    assert_eq!(tombstone.last_seen_release_id, "release-r2");
    assert!(
        home.runtime
            .source_update_is_committed(&record)
            .expect("committed probe includes old tombstone")
    );

    home.runtime
        .undo_source_update(&record)
        .expect("undo update with existing tombstone");
    let restored = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read restored")
        .expect("restored source");
    let tombstone = restored
        .members
        .iter()
        .find(|member| member.skill_id.0 == "skill-b")
        .expect("tombstone after undo");
    assert_eq!(tombstone.presence, SourceMemberPresence::Absent);
    assert_eq!(tombstone.health, Health::Broken);
    assert_eq!(tombstone.last_seen_release_id, RELEASE_R1);
}

#[test]
fn source_update_validate_rejects_changed_current_state() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let record = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-3".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_from_current(&current),
        members: vec![target_member(
            SourceUpdateMemberOrigin::Existing,
            "skill-a",
            "skills/alpha",
            "alpha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )],
        removed_members: vec![SourceUpdateRemovedMemberRecord {
            skill_id: SkillId("skill-b".into()),
            directory_name: "beta".into(),
            skill_path: "skills/beta".into(),
            storage_relpath: "skills/git/remote-source-1/skill-b".into(),
            previous_tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        }],
    };
    // A concurrent mutation (member renamed away) makes the frozen previous
    // set stale.
    home.with_sql("mutate member", |connection| {
        connection
            .execute(
                "UPDATE git_source_members SET skill_path = 'skills/beta-moved'
                  WHERE skill_id = 'skill-b'",
                [],
            )
            .expect("mutate");
    });
    assert!(matches!(
        home.runtime.validate_source_update(&record),
        Err(SourceUpdateStoreError::Conflict(_))
    ));
}

#[test]
fn snapshot_mismatch_health_blocks_update_until_restored() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let record = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-4".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_from_current(&current),
        members: vec![target_member(
            SourceUpdateMemberOrigin::Existing,
            "skill-a",
            "skills/alpha",
            "alpha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )],
        removed_members: vec![SourceUpdateRemovedMemberRecord {
            skill_id: SkillId("skill-b".into()),
            directory_name: "beta".into(),
            skill_path: "skills/beta".into(),
            storage_relpath: "skills/git/remote-source-1/skill-b".into(),
            previous_tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        }],
    };
    home.runtime
        .set_source_member_health(
            REMOTE_ID,
            &[(SkillId("skill-a".into()), Health::SourceSnapshotMismatch)],
        )
        .expect("set mismatch health");
    assert!(matches!(
        home.runtime.validate_source_update(&record),
        Err(SourceUpdateStoreError::Conflict(_))
    ));
    // Restoring health re-opens the Update path.
    home.runtime
        .set_source_member_health(REMOTE_ID, &[(SkillId("skill-a".into()), Health::Healthy)])
        .expect("restore health");
    home.runtime
        .validate_source_update(&record)
        .expect("update allowed after restore");
}

#[test]
fn register_local_copy_allows_git_same_name_but_blocks_non_git() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    // Same Directory Identity as the Git member: no Library conflict.
    home.runtime
        .register_local_copy(&LocalSourceCopyRecord {
            skill_id: SkillId("local-copy-1".into()),
            directory_name: "alpha".into(),
            identity_key: skill_identity_key("alpha"),
            display_name: "Alpha (local copy)".into(),
            description: String::new(),
            final_entity_path: PathBuf::from("/tmp/projects/alpha-copy"),
        })
        .expect("local copy next to Git member");
    // A second local copy with the same identity is a non-Git conflict.
    assert!(matches!(
        home.runtime.register_local_copy(&LocalSourceCopyRecord {
            skill_id: SkillId("local-copy-2".into()),
            directory_name: "alpha".into(),
            identity_key: skill_identity_key("alpha"),
            display_name: String::new(),
            description: String::new(),
            final_entity_path: PathBuf::from("/tmp/projects/alpha-copy-2"),
        }),
        Err(SourceUpdateStoreError::Conflict(_))
    ));
    let rows: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row(
            "SELECT COUNT(*) FROM skills WHERE source_kind = 'link'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(rows, 1);
}

#[test]
fn remove_source_deletes_complete_source_and_cascades_activations() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    // A desired Activation on a member.
    home.with_sql("seed activation", |connection| {
        connection
            .execute_batch(
                r#"
                INSERT INTO global_skill_roots (root_id, configured_path, path_identity_key, created_at, updated_at)
                VALUES ('target-1', '/tmp/agent-skills', 'agent-skills', '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                INSERT INTO activations (
                    skill_id, target_root_id, directory_identity_key, desired_enabled,
                    expected_entry_path, expected_target_path, observed_state
                ) VALUES (
                    'skill-a', 'target-1', 'alpha', 1,
                    'skills/alpha', 'skills/git/remote-source-1/skill-a', 'present'
                );
                "#,
            )
            .expect("seed activation");
    });
    let facts = home
        .runtime
        .source_remove_facts(REMOTE_ID)
        .expect("remove facts");
    assert_eq!(facts.member_skill_ids.len(), 2);
    assert_eq!(facts.activations.len(), 1);
    home.runtime
        .commit_remove_source(REMOTE_ID)
        .expect("commit remove");
    assert!(
        home.runtime
            .source_remove_is_committed(REMOTE_ID)
            .expect("removed probe")
    );
    let remaining: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row(
            "SELECT COUNT(*) FROM skills
              WHERE id IN ('skill-a', 'skill-b')",
            [],
            |row| row.get(0),
        )
        .expect("remaining skills");
    assert_eq!(remaining, 0);
    let remaining_activations: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row("SELECT COUNT(*) FROM activations", [], |row| row.get(0))
        .expect("remaining activations");
    assert_eq!(remaining_activations, 0);
    let parents: i64 = Connection::open(home.catalog_path())
        .expect("open")
        .query_row(
            "SELECT COUNT(*) FROM remote_source_parents WHERE remote_id = ?1",
            [REMOTE_ID],
            |row| row.get(0),
        )
        .expect("parents");
    assert_eq!(parents, 0);
    // Second commit is an idempotent no-op for recovery.
    home.runtime
        .commit_remove_source(REMOTE_ID)
        .expect("idempotent remove");
}

#[test]
fn update_requires_a_target_release_with_unique_members() {
    let home = common::BoundTestHome::new();
    seed_managed_source(&home);
    let current = home
        .runtime
        .read_current(REMOTE_ID)
        .expect("read")
        .expect("source");
    let empty = SourceUpdateRecord {
        remote_id: REMOTE_ID.into(),
        provider: current.provider.clone(),
        canonical_url: current.canonical_url.clone(),
        tracking_mode: "auto_release_tag_head".into(),
        tracking_value: None,
        selection_kind: "tag".into(),
        selected_ref: "v1.1.0".into(),
        release_id: "release-r2".into(),
        resolved_commit: "2222222222222222222222222222222222222222".into(),
        operation_id: "update-5".into(),
        previous_release_id: RELEASE_R1.into(),
        previous_tracking_mode: current.tracking_mode.clone(),
        previous_tracking_value: current.tracking_value.clone(),
        previous_selected_ref: current.selected_ref.clone(),
        previous_resolved_commit: current.resolved_commit.clone(),
        previous_members: previous_from_current(&current),
        members: Vec::new(),
        removed_members: Vec::new(),
    };
    assert!(matches!(
        home.runtime.validate_source_update(&empty),
        Err(SourceUpdateStoreError::Conflict(_))
    ));
}

#[test]
fn read_current_is_none_for_unknown_source() {
    let home = common::BoundTestHome::new();
    assert!(
        home.runtime
            .read_current("never-seen")
            .expect("read")
            .is_none()
    );
}

fn transition_member(
    remote_id: &str,
    skill_id: &str,
    skill_path: &str,
    directory_name: &str,
) -> SourceTransitionMemberRecord {
    SourceTransitionMemberRecord {
        skill_id: SkillId(skill_id.into()),
        directory_name: directory_name.into(),
        identity_key: skill_identity_key(directory_name),
        display_name: directory_name.into(),
        description: String::new(),
        storage_relpath: format!("skills/git/{remote_id}/{skill_id}"),
        skill_path: skill_path.into(),
        tree_hash: "a".repeat(40),
        provider_hash: None,
    }
}

#[test]
fn git_sources_allow_same_directory_identity_within_and_across_sources() {
    let home = common::BoundTestHome::new();
    let same_source = SourceTransitionRecord {
        remote_id: "remote-same".into(),
        provider: "github".into(),
        canonical_url: "https://github.com/acme/same".into(),
        aliases: Vec::new(),
        tracking_mode: "branch".into(),
        tracking_value: Some("main".into()),
        selection_kind: "branch".into(),
        selected_ref: "main".into(),
        release_id: "release-same".into(),
        resolved_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        members: vec![
            transition_member("remote-same", "skill-same-a", "packages/one", "shared"),
            transition_member("remote-same", "skill-same-b", "packages/two", "shared"),
        ],
    };
    home.sqlite
        .validate_new_source_transition(&same_source)
        .expect("same-source duplicate identity is valid");
    home.sqlite
        .commit_source_transition(same_source)
        .expect("commit same-source duplicate identity");

    let other_source = SourceTransitionRecord {
        remote_id: "remote-other".into(),
        provider: "github".into(),
        canonical_url: "https://github.com/acme/other".into(),
        aliases: Vec::new(),
        tracking_mode: "branch".into(),
        tracking_value: Some("main".into()),
        selection_kind: "branch".into(),
        selected_ref: "main".into(),
        release_id: "release-other".into(),
        resolved_commit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        members: vec![transition_member(
            "remote-other",
            "skill-other",
            "skills/shared",
            "shared",
        )],
    };
    home.sqlite
        .validate_new_source_transition(&other_source)
        .expect("cross-source duplicate identity is valid");
    home.sqlite
        .commit_source_transition(other_source)
        .expect("commit cross-source duplicate identity");
}
