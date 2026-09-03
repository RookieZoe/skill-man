//! Schema v9 migration contract (ticket #91, spec §3.4, ADR-0018).
//!
//! v8 → v9 is one transaction: the `skills` Directory Identity constraint is
//! rebuilt (stable `skill_id`, ordinary indexed comparison key), the four v7
//! Git tables are preserved verbatim under `legacy_git_*` names and never
//! guessed into the current model, and the empty v9 Git namespace contract
//! is created. Any failure rolls the whole migration back.

use std::path::Path;

use rusqlite::{Connection, params};
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::seams::catalog_store::StartupAccess;

fn seed_v8_catalog(path: &Path) {
    let connection = Connection::open(path).expect("create v8 catalog");
    connection
        .execute_batch(
            r#"
            CREATE TABLE catalog_meta (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                schema_version INTEGER NOT NULL,
                snapshot_version INTEGER NOT NULL DEFAULT 0,
                first_run_completed_at TEXT,
                last_startup_check_at TEXT,
                home_id TEXT,
                volume_fsid TEXT,
                volume_uuid TEXT,
                home_bound_at TEXT
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                directory_name TEXT NOT NULL,
                identity_key TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                source_kind TEXT NOT NULL,
                library_entry_path TEXT,
                final_entity_path TEXT NOT NULL,
                recorded_content_hash TEXT,
                health TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE agent_configurations (
                agent_id TEXT PRIMARY KEY,
                origin TEXT NOT NULL,
                preset_key TEXT,
                name TEXT NOT NULL,
                name_identity_key TEXT NOT NULL UNIQUE,
                compatibility TEXT NOT NULL,
                project_skills_dir TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE global_skill_roots (
                root_id TEXT PRIMARY KEY,
                configured_path TEXT NOT NULL,
                path_identity_key TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE agent_global_roots (
                agent_id TEXT NOT NULL REFERENCES agent_configurations(agent_id) ON DELETE CASCADE,
                root_id TEXT NOT NULL REFERENCES global_skill_roots(root_id) ON DELETE RESTRICT,
                role TEXT NOT NULL,
                PRIMARY KEY (agent_id, root_id)
            );
            CREATE TABLE activations (
                skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
                target_root_id TEXT NOT NULL REFERENCES global_skill_roots(root_id) ON DELETE RESTRICT,
                directory_identity_key TEXT NOT NULL,
                desired_enabled INTEGER NOT NULL,
                expected_entry_path TEXT NOT NULL,
                expected_target_path TEXT NOT NULL,
                observed_state TEXT NOT NULL,
                last_enabled_at TEXT,
                last_checked_at TEXT,
                PRIMARY KEY (skill_id, target_root_id)
            );
            CREATE TABLE recent_project_folders (
                canonical_path_key TEXT PRIMARY KEY,
                canonical_path TEXT NOT NULL,
                last_used_at TEXT NOT NULL
            );
            CREATE TABLE file_sources (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                original_path TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                installed_at TEXT NOT NULL
            );
            CREATE TABLE remote_source_parents (
                remote_id TEXT PRIMARY KEY,
                canonical_url TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL
            );
            CREATE TABLE remote_source_aliases (
                remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
                alias_url TEXT NOT NULL UNIQUE,
                confirmed_at TEXT NOT NULL,
                PRIMARY KEY (remote_id, alias_url)
            );
            CREATE TABLE remote_bindings (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
                requested_ref TEXT NOT NULL,
                verification_anchor_commit TEXT NOT NULL,
                original_commit_known INTEGER NOT NULL DEFAULT 0,
                skill_path TEXT NOT NULL,
                provider_hash TEXT,
                remote_baseline_hash TEXT NOT NULL,
                current_baseline_hash TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
            );
            CREATE TABLE git_repository_sources (
                remote_id TEXT PRIMARY KEY REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
                provider TEXT NOT NULL,
                canonical_url TEXT NOT NULL,
                tracking_ref TEXT NOT NULL,
                current_release_id TEXT REFERENCES git_source_releases(release_id),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE (provider, canonical_url)
            );
            CREATE TABLE git_source_releases (
                release_id TEXT PRIMARY KEY,
                remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
                tracking_ref TEXT NOT NULL,
                resolved_commit TEXT NOT NULL,
                discovered_at TEXT NOT NULL,
                UNIQUE (remote_id, resolved_commit)
            );
            CREATE TABLE git_source_release_members (
                release_id TEXT NOT NULL REFERENCES git_source_releases(release_id) ON DELETE CASCADE,
                skill_path TEXT NOT NULL,
                skill_name TEXT NOT NULL,
                tree_hash TEXT NOT NULL,
                provider_hash TEXT,
                PRIMARY KEY (release_id, skill_path)
            );
            CREATE TABLE git_source_members (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
                current_skill_path TEXT NOT NULL,
                remote_baseline_hash TEXT NOT NULL,
                current_baseline_hash TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
            );
            CREATE TABLE preferences (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
            );
            INSERT INTO catalog_meta (
                singleton, schema_version, snapshot_version, home_id,
                volume_fsid, volume_uuid, home_bound_at
            ) VALUES (
                1, 8, 3, 'b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab',
                'fsid-1', 'uuid-1', '2026-01-01T00:00:00Z'
            );
            INSERT INTO preferences (singleton) VALUES (1);
            "#,
        )
        .expect("seed v8 schema");

    for (id, directory_name, identity_key, kind, health) in [
        ("skill-link", "review", "review", "link", "healthy"),
        (
            "skill-remote",
            "media-xray",
            "media-xray",
            "remote_install",
            "modified",
        ),
        (
            "skill-git",
            "prompting",
            "prompting",
            "remote_install",
            "healthy",
        ),
    ] {
        if kind == "link" {
            connection
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, final_entity_path, health, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?2, '', ?4, ?2, ?5, '1', '1')",
                    params![id, directory_name, identity_key, kind, health],
                )
                .expect("seed link skill");
        } else {
            connection
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path, health,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?2, '', ?4, ?2, ?2, ?5, '1', '1')",
                    params![id, directory_name, identity_key, kind, health],
                )
                .expect("seed install skill");
        }
    }
    connection
        .execute(
            "INSERT INTO agent_configurations (
                agent_id, origin, name, name_identity_key, compatibility,
                created_at, updated_at
             ) VALUES ('claude', 'preset', 'Claude Code', 'claude code', 'verified', '1', '1')",
            [],
        )
        .expect("seed agent");
    connection
        .execute(
            "INSERT INTO global_skill_roots (
                root_id, configured_path, path_identity_key, created_at, updated_at
             ) VALUES ('root-1', '/home/agent-skills', '/home/agent-skills', '1', '1')",
            [],
        )
        .expect("seed root");
    connection
        .execute(
            "INSERT INTO agent_global_roots (agent_id, root_id, role)
             VALUES ('claude', 'root-1', 'activation_target')",
            [],
        )
        .expect("seed target membership");
    connection
        .execute(
            "INSERT INTO activations (
                skill_id, target_root_id, directory_identity_key, desired_enabled,
                expected_entry_path, expected_target_path, observed_state
             ) VALUES ('skill-link', 'root-1', 'review', 1, '/home/agent-skills/review',
                       '/home/review', 'present')",
            [],
        )
        .expect("seed activation");
    connection
        .execute(
            "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
             VALUES ('parent-1', 'https://github.com/acme/skills.git', '1')",
            [],
        )
        .expect("seed parent");
    connection
        .execute(
            "INSERT INTO remote_source_aliases (remote_id, alias_url, confirmed_at)
             VALUES ('parent-1', 'https://github.com/acme/skills', '1')",
            [],
        )
        .expect("seed alias");
    connection
        .execute(
            "INSERT INTO git_source_releases (
                release_id, remote_id, tracking_ref, resolved_commit, discovered_at
             ) VALUES ('release-1', 'parent-1', 'v1', 'deadbeef', '1')",
            [],
        )
        .expect("seed v7 release");
    connection
        .execute(
            "INSERT INTO git_repository_sources (
                remote_id, provider, canonical_url, tracking_ref, current_release_id,
                created_at, updated_at
             ) VALUES ('parent-1', 'github', 'https://github.com/acme/skills.git', 'v1',
                       'release-1', '1', '1')",
            [],
        )
        .expect("seed v7 git source");
    for (skill_path, skill_name, tree_hash) in [
        ("skills/prompting", "Prompting", "tree-1"),
        ("skills/review-help", "Review help", "tree-2"),
    ] {
        connection
            .execute(
                "INSERT INTO git_source_release_members (
                    release_id, skill_path, skill_name, tree_hash
                 ) VALUES ('release-1', ?1, ?2, ?3)",
                params![skill_path, skill_name, tree_hash],
            )
            .expect("seed v7 release member");
    }
    connection
        .execute(
            "INSERT INTO remote_bindings (
                skill_id, remote_id, requested_ref, verification_anchor_commit,
                original_commit_known, skill_path, remote_baseline_hash,
                current_baseline_hash
             ) VALUES ('skill-remote', 'parent-1', 'v1', 'abc123', 1, 'skills/media-xray',
                       'remote-base', 'current-base')",
            [],
        )
        .expect("seed binding");
    connection
        .execute(
            "INSERT INTO git_source_members (
                skill_id, remote_id, current_skill_path, remote_baseline_hash,
                current_baseline_hash, last_checked_at, last_updated_at
             ) VALUES ('skill-git', 'parent-1', 'skills/prompting', 'old-remote',
                       'old-current', 1, 1)",
            [],
        )
        .expect("seed v7 git member");
}

/// Error-agnostic migration attempt against the seeded catalog: returns the
/// resulting startup status for both success (ReadWrite v9) and failure
/// (ReadOnly, original schema retained) paths.
fn migrate_status(
    path: &Path,
) -> (
    u32,
    StartupAccess,
    skill_man_lib::seams::catalog_store::StartupStatus,
) {
    let store = SqliteCatalogStore::open(path).expect("open catalog for migration");
    let status = store.startup_status();
    (status.schema_version, status.access, status)
}

#[test]
fn fresh_catalog_is_created_at_schema_v9_with_v9_git_contract() {
    let temp = tempfile::tempdir().expect("temp home");
    let catalog_path = temp.path().join("skill-man.sqlite3");

    let store = SqliteCatalogStore::open(&catalog_path).expect("create fresh catalog");
    assert_eq!(store.startup_status().schema_version, 9);
    assert_eq!(store.startup_status().access, StartupAccess::ReadWrite);
    drop(store);

    let connection = Connection::open(&catalog_path).expect("inspect fresh catalog");
    let skills_columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('skills')")
        .expect("skills columns")
        .query_map([], |row| row.get(0))
        .expect("skills columns rows")
        .collect::<Result<_, _>>()
        .expect("skills columns list");
    assert!(skills_columns.contains(&"directory_identity_key".to_string()));
    assert!(!skills_columns.contains(&"identity_key".to_string()));
    assert!(skills_columns.contains(&"id".to_string()));

    let unique_index_on_identity: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_index_list('skills') i
             WHERE i.\"unique\" = 1
               AND EXISTS(
                   SELECT 1 FROM pragma_index_info(i.name) c
                   WHERE c.name = 'directory_identity_key'
               )",
            [],
            |row| row.get(0),
        )
        .expect("unique identity index count");
    assert_eq!(
        unique_index_on_identity, 0,
        "Directory Identity is never a unique constraint"
    );

    let ordinary_identity_index: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_index_list('skills') i
             WHERE i.\"unique\" = 0
               AND EXISTS(
                   SELECT 1 FROM pragma_index_info(i.name) c
                   WHERE c.name = 'directory_identity_key'
               )",
            [],
            |row| row.get(0),
        )
        .expect("ordinary identity index count");
    assert_eq!(
        ordinary_identity_index, 1,
        "Directory Identity stays an indexed comparison key"
    );

    let schema_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'skills'",
            [],
            |row| row.get(0),
        )
        .expect("skills schema sql");
    assert!(
        schema_sql.contains("source_snapshot_mismatch"),
        "skills.health must express Source Snapshot Mismatch: {schema_sql}"
    );

    for (table, column) in [
        ("git_repository_sources", "tracking_mode"),
        ("git_repository_sources", "current_selected_ref"),
        ("git_source_releases", "selection_kind"),
        ("git_source_release_members", "skill_id"),
        ("git_source_members", "storage_relpath"),
        ("git_source_members", "presence"),
    ] {
        let has_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                params![table, column],
                |row| row.get(0),
            )
            .expect("v9 column presence");
        assert_eq!(has_column, 1, "v9 table {table} misses column {column}");
    }

    let member_relpath_unique: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_index_list('git_source_members') i
             WHERE i.\"unique\" = 1",
            [],
            |row| row.get(0),
        )
        .expect("member unique count");
    assert!(
        member_relpath_unique >= 2,
        "v9 members keep both unique constraints"
    );
}

#[test]
fn v8_catalog_migrates_to_v9_preserving_legacy_git_facts_verbatim() {
    let temp = tempfile::tempdir().expect("temp home");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v8_catalog(&catalog_path);

    let (version, access, _) = migrate_status(&catalog_path);
    assert_eq!(version, 9);
    assert_eq!(access, StartupAccess::ReadWrite);

    let connection = Connection::open(&catalog_path).expect("inspect migrated catalog");
    let skill_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
        .expect("skill count");
    assert_eq!(skill_count, 3, "flat Home entities are preserved");
    let preserved: Vec<(String, String)> = connection
        .prepare("SELECT id, directory_identity_key FROM skills ORDER BY id")
        .expect("preserved identities")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("preserved identity rows")
        .collect::<Result<_, _>>()
        .expect("preserved identity list");
    assert_eq!(
        preserved,
        vec![
            ("skill-git".to_string(), "prompting".to_string()),
            ("skill-link".to_string(), "review".to_string()),
            ("skill-remote".to_string(), "media-xray".to_string()),
        ],
        "identity values move to directory_identity_key without guessing"
    );

    let activation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM activations", [], |row| row.get(0))
        .expect("activation count");
    assert_eq!(
        activation_count, 1,
        "skills rebuild must not cascade into activations"
    );
    let binding_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM remote_bindings", [], |row| row.get(0))
        .expect("binding count");
    assert_eq!(binding_count, 1, "Legacy per-Skill bindings are preserved");

    // v9 namespace tables exist and start empty.
    let v9_source_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_repository_sources", [], |row| {
            row.get(0)
        })
        .expect("v9 source count");
    assert_eq!(v9_source_count, 0, "v9 starts empty: no guessed conversion");
    let v9_member_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM git_source_members", [], |row| {
            row.get(0)
        })
        .expect("v9 member count");
    assert_eq!(v9_member_count, 0);

    // Legacy v7 facts survive verbatim under readable legacy names.
    let legacy_source: (String, String, String) = connection
        .query_row(
            "SELECT remote_id, tracking_ref, current_release_id
             FROM legacy_git_repository_sources",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("legacy source row");
    assert_eq!(
        legacy_source,
        (
            "parent-1".to_string(),
            "v1".to_string(),
            "release-1".to_string()
        )
    );
    let legacy_member_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM legacy_git_source_members",
            [],
            |row| row.get(0),
        )
        .expect("legacy member count");
    assert_eq!(legacy_member_count, 1);
    let legacy_member: (String, String, String) = connection
        .query_row(
            "SELECT skill_id, current_skill_path, current_baseline_hash
             FROM legacy_git_source_members",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("legacy member row");
    assert_eq!(
        legacy_member,
        (
            "skill-git".to_string(),
            "skills/prompting".to_string(),
            "old-current".to_string()
        )
    );
    let legacy_member_count_release: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM legacy_git_source_release_members",
            [],
            |row| row.get(0),
        )
        .expect("legacy release member count");
    assert_eq!(legacy_member_count_release, 2);

    // The migrated Catalog is deterministic across restarts.
    drop(connection);
    let (version, _, _) = migrate_status(&catalog_path);
    assert_eq!(
        version, 9,
        "reopen must not re-run or renumber the migration"
    );
}

#[test]
fn v8_catalog_with_partial_git_tables_migrates_without_guessing() {
    let temp = tempfile::tempdir().expect("temp home");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v8_catalog(&catalog_path);
    let connection = Connection::open(&catalog_path).expect("open partial catalog");
    connection
        .execute_batch(
            "DROP TABLE git_source_members;
             DROP TABLE git_repository_sources;
             DROP TABLE git_source_release_members;
             DROP TABLE git_source_releases;",
        )
        .expect("drop git tables into partial v8 shape");
    drop(connection);

    let (version, access, _) = migrate_status(&catalog_path);
    assert_eq!(version, 9);
    assert_eq!(access, StartupAccess::ReadWrite);

    let connection = Connection::open(&catalog_path).expect("inspect partial migration");
    let v9_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'git_repository_sources', 'git_source_releases',
                'git_source_release_members', 'git_source_members'
             )",
            [],
            |row| row.get(0),
        )
        .expect("v9 table count");
    assert_eq!(
        v9_tables, 4,
        "v9 namespace tables are created even from a partial input"
    );
    let legacy_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name LIKE 'legacy_git_%'",
            [],
            |row| row.get(0),
        )
        .expect("legacy table count");
    assert_eq!(
        legacy_tables, 0,
        "absent v7 tables leave no legacy names behind"
    );
}

#[test]
fn v8_catalog_with_orphaned_git_member_rolls_back_the_whole_migration() {
    let temp = tempfile::tempdir().expect("temp home");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v8_catalog(&catalog_path);
    let connection = Connection::open(&catalog_path).expect("open corrupt catalog");
    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for corrupt seed");
    connection
        .execute(
            "INSERT INTO git_source_members (
                skill_id, remote_id, current_skill_path, remote_baseline_hash,
                current_baseline_hash
             ) VALUES ('no-such-skill', 'parent-1', 'skills/ghost', 'a', 'b')",
            [],
        )
        .expect("seed orphaned member with foreign_keys OFF");
    drop(connection);

    let (version, access, _) = migrate_status(&catalog_path);
    assert_eq!(
        version, 8,
        "a failing migration leaves the complete v8 shape intact"
    );
    assert_eq!(access, StartupAccess::ReadOnly);

    let connection = Connection::open(&catalog_path).expect("inspect rolled-back catalog");
    let v9_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'git_repository_sources', 'git_source_releases',
                'git_source_release_members', 'git_source_members',
                'legacy_git_repository_sources', 'legacy_git_source_releases',
                'legacy_git_source_release_members', 'legacy_git_source_members'
             )",
            [],
            |row| row.get(0),
        )
        .expect("v9 table count");
    assert_eq!(
        v9_tables, 4,
        "no partial v9 or legacy shape survives a rollback"
    );
    let identity_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'identity_key'",
            [],
            |row| row.get(0),
        )
        .expect("identity column presence");
    assert_eq!(
        identity_column, 1,
        "the v8 skills shape is intact after rollback"
    );
}
