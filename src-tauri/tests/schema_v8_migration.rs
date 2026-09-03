use std::path::Path;

use rusqlite::{Connection, params};
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;

fn seed_v7_catalog(path: &Path, target_path: &Path, alias_path: &Path) {
    let connection = Connection::open(path).expect("create v7 catalog");
    connection
        .execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
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
            CREATE TABLE agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                skills_path TEXT NOT NULL,
                path_identity_key TEXT NOT NULL UNIQUE,
                detected INTEGER NOT NULL,
                compatibility TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE activations (
                skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
                agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                desired_enabled INTEGER NOT NULL,
                expected_entry_path TEXT NOT NULL UNIQUE,
                expected_target_path TEXT NOT NULL,
                observed_state TEXT NOT NULL,
                last_enabled_at TEXT,
                last_checked_at TEXT,
                PRIMARY KEY (skill_id, agent_id)
            );
            INSERT INTO catalog_meta (
                singleton, schema_version, snapshot_version, home_id,
                volume_fsid, volume_uuid, home_bound_at
            ) VALUES (
                1, 7, 12, 'b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab',
                'fsid-1', 'uuid-1', '2026-01-01T00:00:00Z'
            );
            "#,
        )
        .expect("seed v7 schema");

    let entity_path = target_path.parent().expect("target parent").join("entity");
    std::fs::create_dir_all(&entity_path).expect("create entity");
    connection
        .execute(
            "INSERT INTO skills (
                id, directory_name, identity_key, display_name, source_kind,
                final_entity_path, health, created_at, updated_at
             ) VALUES ('skill-1', 'review', 'review', 'Review', 'link', ?1,
                       'healthy', '1', '1')",
            [entity_path.to_string_lossy().as_ref()],
        )
        .expect("seed skill");

    for (id, name, kind, configured_path, identity) in [
        (
            "claude",
            "Claude Code",
            "claude_preset",
            target_path,
            "legacy-target-a",
        ),
        ("cursor", "Cursor", "custom", alias_path, "legacy-target-b"),
    ] {
        connection
            .execute(
                "INSERT INTO agents (
                    id, name, kind, skills_path, path_identity_key, detected,
                    compatibility, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, 'verified', '1', '1')",
                params![id, name, kind, configured_path.to_string_lossy(), identity],
            )
            .expect("seed agent");
    }

    connection
        .execute(
            "INSERT INTO activations (
                skill_id, agent_id, desired_enabled, expected_entry_path,
                expected_target_path, observed_state, last_enabled_at, last_checked_at
             ) VALUES ('skill-1', 'claude', 1, ?1, ?2, 'present', '2', '3')",
            params![
                target_path.join("review").to_string_lossy(),
                entity_path.to_string_lossy()
            ],
        )
        .expect("seed activation");
}

fn seed_alias_activation(path: &Path, alias_path: &Path, desired_enabled: bool) {
    let connection = Connection::open(path).expect("open v7 catalog");
    let expected_target_path: String = connection
        .query_row(
            "SELECT final_entity_path FROM skills WHERE id = 'skill-1'",
            [],
            |row| row.get(0),
        )
        .expect("skill target");
    connection
        .execute(
            "INSERT INTO activations (
                skill_id, agent_id, desired_enabled, expected_entry_path,
                expected_target_path, observed_state, last_enabled_at, last_checked_at
             ) VALUES ('skill-1', 'cursor', ?1, ?2, ?3, 'present', '4', '5')",
            params![
                desired_enabled,
                alias_path.join("review").to_string_lossy(),
                expected_target_path
            ],
        )
        .expect("seed aliased activation");
}

#[test]
fn v7_migration_groups_aliases_by_canonical_target_and_drops_detection_state() {
    let temp = tempfile::tempdir().expect("temp home");
    let target = temp.path().join("agent-skills");
    let alias = temp.path().join("agent-skills-alias");
    std::fs::create_dir_all(&target).expect("create target");
    std::os::unix::fs::symlink(&target, &alias).expect("create target alias");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v7_catalog(&catalog_path, &target, &alias);

    let store = SqliteCatalogStore::open(&catalog_path).expect("migrate v7 catalog");
    assert_eq!(
        store.startup_status().schema_version,
        9,
        "v7 catalogs migrate through v8 to the current v9 contract"
    );
    drop(store);

    let connection = Connection::open(&catalog_path).expect("inspect migrated catalog");
    let legacy_table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'agents'",
            [],
            |row| row.get(0),
        )
        .expect("legacy table count");
    assert_eq!(legacy_table_count, 0);

    let configuration_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_configurations", [], |row| {
            row.get(0)
        })
        .expect("configuration count");
    let root_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM global_skill_roots", [], |row| {
            row.get(0)
        })
        .expect("root count");
    let target_membership_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_global_roots WHERE role = 'activation_target'",
            [],
            |row| row.get(0),
        )
        .expect("target membership count");
    assert_eq!(configuration_count, 2);
    assert_eq!(root_count, 1, "canonical aliases share one physical root");
    assert_eq!(target_membership_count, 2);

    let activation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM activations", [], |row| row.get(0))
        .expect("activation count");
    let target_matches_membership: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM activations a
                JOIN agent_global_roots r
                  ON r.root_id = a.target_root_id
                 AND r.role = 'activation_target'
            )",
            [],
            |row| row.get(0),
        )
        .expect("activation target membership");
    assert_eq!(activation_count, 1);
    assert!(target_matches_membership);

    let recent_table_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'recent_project_folders'",
            [],
            |row| row.get(0),
        )
        .expect("recent table count");
    assert_eq!(recent_table_count, 1);
}

#[test]
fn ambiguous_target_aggregation_rolls_back_whole_migration_then_can_roll_forward() {
    let temp = tempfile::tempdir().expect("temp home");
    let target = temp.path().join("agent-skills");
    let alias = temp.path().join("agent-skills-alias");
    std::fs::create_dir_all(&target).expect("create target");
    std::os::unix::fs::symlink(&target, &alias).expect("create target alias");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v7_catalog(&catalog_path, &target, &alias);
    seed_alias_activation(&catalog_path, &alias, false);

    let store = SqliteCatalogStore::open(&catalog_path).expect("open failed migration read-only");
    let status = store.startup_status();
    assert_eq!(status.schema_version, 7);
    assert_eq!(
        status.access,
        skill_man_lib::seams::catalog_store::StartupAccess::ReadOnly
    );
    drop(store);

    let connection = Connection::open(&catalog_path).expect("inspect rolled-back catalog");
    let tables: (i64, i64) = connection
        .query_row(
            "SELECT
                EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agents'),
                EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'agent_configurations'
                )",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("migration table state");
    assert_eq!(
        tables,
        (1, 0),
        "failed migration leaves the v7 shape intact"
    );
    connection
        .execute(
            "UPDATE activations
             SET desired_enabled = 1
             WHERE skill_id = 'skill-1' AND agent_id = 'cursor'",
            [],
        )
        .expect("repair ambiguous legacy fact");
    drop(connection);

    let store = SqliteCatalogStore::open(&catalog_path).expect("roll migration forward");
    assert_eq!(
        store.startup_status().schema_version,
        9,
        "v7 catalogs migrate through v8 to the current v9 contract"
    );
    drop(store);
    let connection = Connection::open(&catalog_path).expect("inspect rolled-forward catalog");
    let activation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM activations", [], |row| row.get(0))
        .expect("activation count");
    assert_eq!(
        activation_count, 1,
        "shared physical Target aggregates once"
    );
}

#[test]
fn duplicate_case_fold_names_fail_closed_without_partial_migration() {
    let temp = tempfile::tempdir().expect("temp home");
    let target = temp.path().join("agent-skills");
    std::fs::create_dir_all(&target).expect("create target");
    let catalog_path = temp.path().join("skill-man.sqlite3");
    seed_v7_catalog(&catalog_path, &target, &target);

    let connection = Connection::open(&catalog_path).expect("open catalog");
    connection
        .execute(
            "INSERT INTO agents (
                id, name, kind, skills_path, path_identity_key, detected,
                compatibility, created_at, updated_at
             ) VALUES (
                'mirror-a', 'ＣＵＲＳＯＲ', 'custom', ?1, 'mirror-target-a',
                1, 'unknown', '1', '1'
             )",
            params![target.to_string_lossy()],
        )
        .expect("seed case-fold duplicate Agent");
    drop(connection);

    let store = SqliteCatalogStore::open(&catalog_path).expect("open failed migration read-only");
    assert_eq!(store.startup_status().schema_version, 7);
    assert_eq!(
        store.startup_status().access,
        skill_man_lib::seams::catalog_store::StartupAccess::ReadOnly
    );

    let connection = Connection::open(&catalog_path).expect("inspect rolled-back catalog");
    let legacy_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
        .expect("legacy agents count");
    let configuration_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'agent_configurations', 'global_skill_roots', 'agent_global_roots'
             )",
            [],
            |row| row.get(0),
        )
        .expect("v8 table count");
    assert_eq!(legacy_count, 3);
    assert_eq!(configuration_tables, 0, "no partial v8 shape survives");
}
