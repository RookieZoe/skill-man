use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentId, AgentKind, CatalogSeed, Compatibility, Health, SkillId,
    SourceKind,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
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

    pub fn seed_catalog_if_empty(&self, seed: &CatalogSeed) -> Result<(), ActivationStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_activation_error)?;
        let skill_count: i64 = transaction
            .query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
            .map_err(sqlite_activation_error)?;
        if skill_count != 0 {
            transaction.commit().map_err(sqlite_activation_error)?;
            return Ok(());
        }

        for skill in &seed.skills {
            let summary = &skill.summary;
            let library_entry_path = if summary.source_kind == SourceKind::Link {
                None
            } else {
                Some(skill.final_entity_path.as_str())
            };
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path, health,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                    params![
                        summary.id.0,
                        summary.directory_name,
                        summary.directory_name.to_lowercase(),
                        summary.display_name,
                        summary.description,
                        source_kind_value(summary.source_kind),
                        library_entry_path,
                        skill.final_entity_path,
                        health_value(summary.health),
                        skill.last_activity_at,
                    ],
                )
                .map_err(sqlite_activation_error)?;
        }
        for agent in &seed.agents {
            transaction
                .execute(
                    "INSERT INTO agents (
                        id, name, kind, skills_path, path_identity_key, detected,
                        compatibility, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    params![
                        agent.id.0,
                        agent.name,
                        agent_kind_value(agent.kind),
                        agent.skills_path,
                        agent.skills_path.to_lowercase(),
                        agent.detected,
                        compatibility_value(agent.compatibility),
                        "1970-01-01T00:00:00Z",
                    ],
                )
                .map_err(sqlite_activation_error)?;
        }
        let snapshot_version = i64::try_from(seed.snapshot_version).map_err(|_| {
            ActivationStoreError::Unavailable(
                "fixture snapshot version exceeds SQLite range".into(),
            )
        })?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = ?1 WHERE singleton = 1",
                [snapshot_version],
            )
            .map_err(sqlite_activation_error)?;
        transaction.commit().map_err(sqlite_activation_error)
    }

    pub fn persisted_snapshot_version(&self) -> Result<u64, ActivationStoreError> {
        let value: i64 = self
            .connection()?
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_activation_error)?;
        u64::try_from(value).map_err(|_| {
            ActivationStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    pub fn enabled_count(&self, skill_id: &SkillId) -> Result<u32, ActivationStoreError> {
        self.connection()?
            .query_row(
                "SELECT COUNT(*) FROM activations WHERE skill_id = ?1 AND desired_enabled = 1",
                [&skill_id.0],
                |row| row.get(0),
            )
            .map_err(sqlite_activation_error)
    }

    pub fn persisted_activation(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<Option<PersistedActivation>, ActivationStoreError> {
        self.connection()?
            .query_row(
                "SELECT desired_enabled, observed_state FROM activations
                 WHERE skill_id = ?1 AND agent_id = ?2",
                params![skill_id.0, agent_id.0],
                |row| {
                    Ok(PersistedActivation {
                        desired_enabled: row.get(0)?,
                        observed_state: parse_observed_state(&row.get::<_, String>(1)?)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_activation_error)
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, ActivationStoreError> {
        self.connection
            .lock()
            .map_err(|_| ActivationStoreError::Unavailable("SQLite lock poisoned".into()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistedActivation {
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedState,
}

impl ActivationStore for SqliteCatalogStore {
    fn load(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        self.connection()?
            .query_row(
                "SELECT
                    skills.directory_name, skills.final_entity_path,
                    agents.name, agents.kind, agents.skills_path,
                    COALESCE(activations.desired_enabled, 0),
                    activations.expected_target_path
                 FROM skills
                 JOIN agents ON agents.id = ?2
                 LEFT JOIN activations
                    ON activations.skill_id = skills.id AND activations.agent_id = agents.id
                 WHERE skills.id = ?1",
                params![skill_id.0, agent_id.0],
                |row| {
                    Ok(ActivationContext {
                        skill_id: skill_id.clone(),
                        directory_name: row.get(0)?,
                        final_entity_path: PathBuf::from(row.get::<_, String>(1)?),
                        agent_id: agent_id.clone(),
                        agent_name: row.get(2)?,
                        agent_kind: parse_agent_kind(&row.get::<_, String>(3)?)?,
                        agent_skills_path: PathBuf::from(row.get::<_, String>(4)?),
                        desired_enabled: row.get(5)?,
                        expected_target_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                    })
                },
            )
            .optional()
            .map_err(sqlite_activation_error)
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT id, skills_path FROM agents ORDER BY id")
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([], |row| {
                Ok(ConfiguredAgentPath {
                    agent_id: AgentId(row.get(0)?),
                    skills_path: PathBuf::from(row.get::<_, String>(1)?),
                })
            })
            .map_err(sqlite_activation_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_activation_error)
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_activation_error)?;
        transaction
            .execute(
                "INSERT INTO activations (
                    skill_id, agent_id, desired_enabled, expected_entry_path,
                    expected_target_path, observed_state, last_enabled_at, last_checked_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                 ON CONFLICT(skill_id, agent_id) DO UPDATE SET
                    desired_enabled = excluded.desired_enabled,
                    expected_entry_path = excluded.expected_entry_path,
                    expected_target_path = excluded.expected_target_path,
                    observed_state = excluded.observed_state,
                    last_enabled_at = CASE
                        WHEN excluded.desired_enabled = 1 THEN excluded.last_enabled_at
                        ELSE activations.last_enabled_at
                    END,
                    last_checked_at = excluded.last_checked_at",
                params![
                    record.skill_id.0,
                    record.agent_id.0,
                    record.desired_enabled,
                    record.expected_entry_path.to_string_lossy(),
                    record.expected_target_path.to_string_lossy(),
                    observed_state_value(record.observed_state),
                    unix_timestamp(),
                ],
            )
            .map_err(sqlite_activation_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_activation_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_activation_error)?;
        transaction.commit().map_err(sqlite_activation_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            ActivationStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT skill_id, agent_id, expected_entry_path, expected_target_path
                 FROM activations
                 WHERE desired_enabled = 1
                 ORDER BY skill_id, agent_id",
            )
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([], |row| {
                Ok(DesiredActivation {
                    skill_id: SkillId(row.get(0)?),
                    agent_id: AgentId(row.get(1)?),
                    expected_entry_path: PathBuf::from(row.get::<_, String>(2)?),
                    expected_target_path: PathBuf::from(row.get::<_, String>(3)?),
                })
            })
            .map_err(sqlite_activation_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_activation_error)
    }

    fn record_observations(
        &self,
        observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_activation_error)?;
        let checked_at = unix_timestamp();
        for observation in observations {
            transaction
                .execute(
                    "UPDATE activations
                     SET observed_state = ?1, last_checked_at = ?2
                     WHERE skill_id = ?3 AND agent_id = ?4 AND desired_enabled = 1",
                    params![
                        observed_state_value(observation.observed_state),
                        checked_at,
                        observation.skill_id.0,
                        observation.agent_id.0,
                    ],
                )
                .map_err(sqlite_activation_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta
                 SET snapshot_version = snapshot_version + 1, last_startup_check_at = ?1
                 WHERE singleton = 1",
                [checked_at],
            )
            .map_err(sqlite_activation_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_activation_error)?;
        transaction.commit().map_err(sqlite_activation_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            ActivationStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    fn record_observation(
        &self,
        observation: &ActivationObservation,
    ) -> Result<u64, ActivationStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_activation_error)?;
        transaction
            .execute(
                "UPDATE activations
                 SET observed_state = ?1, last_checked_at = ?2
                 WHERE skill_id = ?3 AND agent_id = ?4 AND desired_enabled = 1",
                params![
                    observed_state_value(observation.observed_state),
                    unix_timestamp(),
                    observation.skill_id.0,
                    observation.agent_id.0,
                ],
            )
            .map_err(sqlite_activation_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_activation_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_activation_error)?;
        transaction.commit().map_err(sqlite_activation_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            ActivationStoreError::Unavailable("negative SQLite snapshot version".into())
        })
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

fn sqlite_activation_error(error: rusqlite::Error) -> ActivationStoreError {
    ActivationStoreError::Unavailable(error.to_string())
}

fn source_kind_value(value: SourceKind) -> &'static str {
    match value {
        SourceKind::Link => "link",
        SourceKind::RemoteInstall => "remote_install",
        SourceKind::FileInstall => "file_install",
    }
}

fn health_value(value: Health) -> &'static str {
    match value {
        Health::Healthy => "healthy",
        Health::Broken => "broken",
        Health::Modified => "modified",
    }
}

fn agent_kind_value(value: AgentKind) -> &'static str {
    match value {
        AgentKind::ClaudePreset => "claude_preset",
        AgentKind::CodexPreset => "codex_preset",
        AgentKind::Custom => "custom",
    }
}

fn parse_agent_kind(value: &str) -> rusqlite::Result<AgentKind> {
    match value {
        "claude_preset" => Ok(AgentKind::ClaudePreset),
        "codex_preset" => Ok(AgentKind::CodexPreset),
        "custom" => Ok(AgentKind::Custom),
        value => Err(invalid_enum_value(3, value)),
    }
}

fn compatibility_value(value: Compatibility) -> &'static str {
    match value {
        Compatibility::Verified => "verified",
        Compatibility::Unknown => "unknown",
    }
}

fn observed_state_value(value: ActivationObservedState) -> &'static str {
    match value {
        ActivationObservedState::Present => "present",
        ActivationObservedState::Missing => "missing",
        ActivationObservedState::TargetMismatch => "target_mismatch",
        ActivationObservedState::Dangling => "dangling",
        ActivationObservedState::Occupied => "occupied",
    }
}

fn parse_observed_state(value: &str) -> rusqlite::Result<ActivationObservedState> {
    match value {
        "present" => Ok(ActivationObservedState::Present),
        "missing" => Ok(ActivationObservedState::Missing),
        "target_mismatch" => Ok(ActivationObservedState::TargetMismatch),
        "dangling" => Ok(ActivationObservedState::Dangling),
        "occupied" => Ok(ActivationObservedState::Occupied),
        value => Err(invalid_enum_value(1, value)),
    }
}

fn invalid_enum_value(column: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("invalid persisted enum value '{value}'").into(),
    )
}

fn unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
