use std::sync::Arc;

use rusqlite::Connection;
use skill_man_lib::adapters::fixture_catalog::FixtureCatalogStore;
use skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore;
use skill_man_lib::adapters::sqlite::{CURRENT_SCHEMA_VERSION, SqliteCatalogStore};
use skill_man_lib::core::catalog::CatalogService;
use skill_man_lib::core::domain::{AgentId, SkillId};
use skill_man_lib::seams::activation_store::ActivationStore;
use skill_man_lib::seams::catalog_store::{StartupAccess, StartupDiagnosticCode};
use skill_man_lib::tauri_adapter::catalog_api::CatalogApi;
use skill_man_lib::tauri_adapter::dto::{CatalogFilterDto, ListSkillsRequestDto};

#[test]
fn a_new_catalog_opens_writable_at_the_current_schema() {
    let home = tempfile::tempdir().expect("temporary Library root");
    let database_path = home.path().join("skill-man.sqlite3");

    let store = SqliteCatalogStore::open(&database_path).expect("catalog opens");
    let status = store.startup_status();

    assert_eq!(status.access, StartupAccess::ReadWrite);
    assert_eq!(status.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(status.diagnostic, None);
}

#[test]
fn an_unsupported_future_schema_opens_read_only_with_a_diagnostic() {
    let home = tempfile::tempdir().expect("temporary Library root");
    let database_path = home.path().join("skill-man.sqlite3");
    let connection = Connection::open(&database_path).expect("create future catalog");
    connection
        .execute_batch(
            "CREATE TABLE catalog_meta (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                schema_version INTEGER NOT NULL,
                snapshot_version INTEGER NOT NULL
            );
            INSERT INTO catalog_meta VALUES (1, 99, 0);",
        )
        .expect("seed future catalog");
    drop(connection);

    let store = SqliteCatalogStore::open(&database_path).expect("catalog opens read-only");
    let status = store.startup_status();

    assert_eq!(status.access, StartupAccess::ReadOnly);
    assert_eq!(status.schema_version, 99);
    assert_eq!(
        status.diagnostic.expect("diagnostic").code,
        StartupDiagnosticCode::UnsupportedSchema
    );
    assert_read_only_runtime_browses_fixture(home.path(), store);
}

#[test]
fn a_failed_migration_restores_the_original_catalog_and_locks_writes() {
    let home = tempfile::tempdir().expect("temporary Library root");
    let database_path = home.path().join("skill-man.sqlite3");
    let connection = Connection::open(&database_path).expect("create legacy catalog");
    connection
        .execute("CREATE TABLE skills (legacy_name TEXT NOT NULL)", [])
        .expect("seed incompatible legacy table");
    drop(connection);

    let store = SqliteCatalogStore::open(&database_path).expect("catalog opens read-only");
    let status = store.startup_status();

    assert_eq!(status.access, StartupAccess::ReadOnly);
    assert_eq!(status.schema_version, 0);
    let diagnostic = status.diagnostic.expect("diagnostic");
    assert_eq!(diagnostic.code, StartupDiagnosticCode::MigrationFailed);
    assert!(
        diagnostic
            .backup_path
            .as_deref()
            .is_some_and(|path| std::path::Path::new(path).is_file()),
        "an existing catalog is backed up before migration"
    );
    assert_read_only_runtime_browses_fixture(home.path(), store);
}

#[test]
fn a_migration_backup_includes_committed_wal_content() {
    let home = tempfile::tempdir().expect("temporary Library root");
    let database_path = home.path().join("skill-man.sqlite3");
    let connection = Connection::open(&database_path).expect("create WAL catalog");
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL");
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("keep committed pages in WAL");
    connection
        .execute_batch(
            "CREATE TABLE skills (legacy_name TEXT NOT NULL);
             INSERT INTO skills VALUES ('committed-in-wal');",
        )
        .expect("seed incompatible WAL catalog");

    let store = SqliteCatalogStore::open(&database_path).expect("catalog opens read-only");
    let backup_path = store
        .startup_status()
        .diagnostic
        .expect("migration diagnostic")
        .backup_path
        .expect("migration backup");
    let backup =
        Connection::open_with_flags(backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open migration backup");
    let legacy_name: String = backup
        .query_row("SELECT legacy_name FROM skills", [], |row| row.get(0))
        .expect("read committed WAL content from backup");

    assert_eq!(legacy_name, "committed-in-wal");
}

fn assert_read_only_runtime_browses_fixture(
    library_root: &std::path::Path,
    sqlite: SqliteCatalogStore,
) {
    let runtime = Arc::new(RuntimeCatalogStore::new(
        Arc::new(FixtureCatalogStore::library_desk()),
        Arc::new(sqlite),
    ));
    let catalog = CatalogApi::new(CatalogService::new(runtime.clone()));

    let result = catalog
        .list_skills(ListSkillsRequestDto {
            filter: CatalogFilterDto::All,
        })
        .expect("read-only runtime keeps fixture browsing available");

    assert_eq!(result.items.len(), 3);
    assert_eq!(result.snapshot_version, 7);
    assert!(
        runtime
            .load(
                &SkillId("skill-authoring".into()),
                &AgentId("claude-code".into())
            )
            .is_err(),
        "Activation writes remain unavailable in read-only startup"
    );
    assert!(
        !library_root.join("fixture-entities").exists(),
        "read-only startup does not materialize fixture entities"
    );
}
