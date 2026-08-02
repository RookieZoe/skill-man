use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use thiserror::Error;

use crate::seams::catalog_store::{
    StartupAccess, StartupDiagnostic, StartupDiagnosticCode, StartupStatus,
};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

const INITIAL_SCHEMA: &str = r#"
CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL,
    snapshot_version INTEGER NOT NULL DEFAULT 0,
    last_startup_check_at TEXT
);

CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    directory_name TEXT NOT NULL,
    identity_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    source_kind TEXT NOT NULL CHECK (source_kind IN ('link', 'remote_install', 'file_install')),
    library_entry_path TEXT,
    final_entity_path TEXT NOT NULL,
    recorded_content_hash TEXT,
    health TEXT NOT NULL CHECK (health IN ('healthy', 'broken', 'modified')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (source_kind = 'link' AND library_entry_path IS NULL)
        OR (source_kind != 'link' AND library_entry_path IS NOT NULL)
    )
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('claude_preset', 'codex_preset', 'custom')),
    skills_path TEXT NOT NULL,
    path_identity_key TEXT NOT NULL UNIQUE,
    detected INTEGER NOT NULL CHECK (detected IN (0, 1)),
    compatibility TEXT NOT NULL CHECK (compatibility IN ('verified', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE activations (
    skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    desired_enabled INTEGER NOT NULL CHECK (desired_enabled IN (0, 1)),
    expected_entry_path TEXT NOT NULL UNIQUE,
    expected_target_path TEXT NOT NULL,
    observed_state TEXT NOT NULL CHECK (
        observed_state IN ('present', 'missing', 'target_mismatch', 'dangling', 'occupied')
    ),
    last_enabled_at TEXT,
    last_checked_at TEXT,
    PRIMARY KEY (skill_id, agent_id)
);

CREATE TABLE preferences (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    launch_at_login INTEGER NOT NULL DEFAULT 0 CHECK (launch_at_login IN (0, 1)),
    show_in_dock INTEGER NOT NULL DEFAULT 1 CHECK (show_in_dock IN (0, 1)),
    check_app_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_app_updates IN (0, 1)),
    check_skill_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_skill_updates IN (0, 1)),
    last_app_update_check_at TEXT,
    last_skill_update_check_at TEXT
);

INSERT INTO catalog_meta (singleton, schema_version, snapshot_version)
VALUES (1, 1, 0);

INSERT INTO preferences (singleton) VALUES (1);
"#;

#[derive(Debug, Error)]
pub enum CatalogStoreOpenError {
    #[error("could not prepare the Library directory: {0}")]
    PrepareLibrary(#[source] std::io::Error),
    #[error("could not open the SQLite catalog: {0}")]
    Open(#[source] rusqlite::Error),
    #[error("could not configure the SQLite catalog: {0}")]
    Configure(#[source] rusqlite::Error),
    #[error("could not back up the SQLite catalog: {0}")]
    Backup(#[source] rusqlite::Error),
}

pub struct SqliteCatalogStore {
    #[allow(dead_code)]
    connection: Mutex<Connection>,
    startup_status: StartupStatus,
}

impl SqliteCatalogStore {
    pub fn open(path: &Path) -> Result<Self, CatalogStoreOpenError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(CatalogStoreOpenError::PrepareLibrary)?;
        }

        let existing_schema_version = detect_schema_version(path);
        if existing_schema_version > CURRENT_SCHEMA_VERSION {
            let connection = open_read_only(path)?;
            return Ok(Self {
                connection: Mutex::new(connection),
                startup_status: StartupStatus {
                    access: StartupAccess::ReadOnly,
                    schema_version: existing_schema_version,
                    diagnostic: Some(StartupDiagnostic {
                        code: StartupDiagnosticCode::UnsupportedSchema,
                        message: format!(
                            "Catalog schema {existing_schema_version} is newer than supported schema {CURRENT_SCHEMA_VERSION}."
                        ),
                        backup_path: None,
                    }),
                },
            });
        }

        let backup_path = if existing_schema_version < CURRENT_SCHEMA_VERSION && has_content(path) {
            Some(backup_catalog(path, existing_schema_version)?)
        } else {
            None
        };

        let mut connection = Connection::open(path).map_err(CatalogStoreOpenError::Open)?;
        configure_connection(&connection)?;

        if existing_schema_version < CURRENT_SCHEMA_VERSION {
            if let Err(error) = migrate_to_current(&mut connection, existing_schema_version) {
                connection
                    .pragma_update(None, "query_only", true)
                    .map_err(CatalogStoreOpenError::Configure)?;
                return Ok(Self {
                    connection: Mutex::new(connection),
                    startup_status: StartupStatus {
                        access: StartupAccess::ReadOnly,
                        schema_version: existing_schema_version,
                        diagnostic: Some(StartupDiagnostic {
                            code: StartupDiagnosticCode::MigrationFailed,
                            message: format!("Catalog migration failed: {error}"),
                            backup_path: backup_path.map(path_to_string),
                        }),
                    },
                });
            }
        }

        Ok(Self {
            connection: Mutex::new(connection),
            startup_status: StartupStatus {
                access: StartupAccess::ReadWrite,
                schema_version: CURRENT_SCHEMA_VERSION,
                diagnostic: None,
            },
        })
    }

    pub fn startup_status(&self) -> StartupStatus {
        self.startup_status.clone()
    }
}

fn configure_connection(connection: &Connection) -> Result<(), CatalogStoreOpenError> {
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(CatalogStoreOpenError::Configure)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(CatalogStoreOpenError::Configure)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(CatalogStoreOpenError::Configure)
}

fn open_read_only(path: &Path) -> Result<Connection, CatalogStoreOpenError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(CatalogStoreOpenError::Open)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(CatalogStoreOpenError::Configure)?;
    Ok(connection)
}

fn detect_schema_version(path: &Path) -> u32 {
    if !has_content(path) {
        return 0;
    }

    let Ok(connection) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return 0;
    };
    let has_meta = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'catalog_meta')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if !has_meta {
        return 0;
    }

    connection
        .query_row(
            "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

fn migrate_to_current(
    connection: &mut Connection,
    existing_schema_version: u32,
) -> rusqlite::Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if existing_schema_version == 0 {
        transaction.execute_batch(INITIAL_SCHEMA)?;
    }
    transaction.commit()
}

fn backup_catalog(path: &Path, schema_version: u32) -> Result<PathBuf, CatalogStoreOpenError> {
    let extension = format!("pre-migration-v{schema_version}.bak");
    let backup_path = path.with_extension(extension);
    let source = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(CatalogStoreOpenError::Backup)?;
    let mut destination = Connection::open(&backup_path).map_err(CatalogStoreOpenError::Backup)?;
    rusqlite::backup::Backup::new(&source, &mut destination)
        .and_then(|backup| backup.run_to_completion(128, Duration::from_millis(10), None))
        .map_err(CatalogStoreOpenError::Backup)?;
    Ok(backup_path)
}

fn has_content(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.len() > 0)
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}
