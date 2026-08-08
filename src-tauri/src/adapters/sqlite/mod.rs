use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentId, AgentKind, CatalogFilter, CatalogSeed,
    Compatibility, Health, SkillId, SkillSummary, SourceKind, skill_identity_key,
};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord,
    LibraryConflict as AdoptLibraryConflict,
};
use crate::seams::catalog_store::{
    CatalogStoreError, StartupAccess, StartupDiagnostic, StartupDiagnosticCode, StartupStatus,
};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
    RemoteImportRecord, RemoteInstallRecord,
};
use crate::seams::maintenance_store::{
    AdoptedSkillEntity, InstalledSkillBaseline, LinkSkillRecord, MaintenanceStoreError,
    ManagedSkillBaseline, RelocateActivationBaseline, SkillHealthObservation,
};
use crate::seams::preferences_store::PreferencesStoreError;

pub const CURRENT_SCHEMA_VERSION: u32 = 4;

const INITIAL_SCHEMA: &str = r#"
CREATE TABLE catalog_meta (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL,
    snapshot_version INTEGER NOT NULL DEFAULT 0,
    first_run_completed_at TEXT,
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

CREATE TABLE file_sources (
    skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
    original_path TEXT NOT NULL,
    original_filename TEXT NOT NULL,
    installed_at TEXT NOT NULL
);

CREATE TABLE remote_sources (
    skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
    source_url TEXT NOT NULL,
    requested_ref TEXT NOT NULL,
    resolved_commit TEXT NOT NULL,
    skill_path TEXT NOT NULL,
    last_checked_at INTEGER,
    last_updated_at INTEGER
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
VALUES (1, 4, 0);

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
                        skill_identity_key(&summary.directory_name),
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

    pub fn list_skill_summaries(
        &self,
        filter: CatalogFilter,
    ) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        let connection = self
            .connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?;
        let mut statement = connection
            .prepare(
                "SELECT
                    skills.id, skills.directory_name, skills.display_name,
                    skills.description, skills.source_kind, skills.health,
                    COALESCE(SUM(CASE WHEN activations.desired_enabled = 1 THEN 1 ELSE 0 END), 0)
                 FROM skills
                 LEFT JOIN activations ON activations.skill_id = skills.id
                 GROUP BY skills.id
                 ORDER BY skills.updated_at DESC, skills.directory_name COLLATE NOCASE, skills.id",
            )
            .map_err(sqlite_catalog_error)?;
        let skills = statement
            .query_map([], |row| {
                Ok(SkillSummary {
                    id: SkillId(row.get(0)?),
                    directory_name: row.get(1)?,
                    display_name: row.get(2)?,
                    description: row.get(3)?,
                    source_kind: parse_source_kind(&row.get::<_, String>(4)?)?,
                    health: parse_health(&row.get::<_, String>(5)?)?,
                    enabled_agent_count: row.get(6)?,
                })
            })
            .map_err(sqlite_catalog_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_catalog_error)?;
        Ok(skills
            .into_iter()
            .filter(|skill| filter.includes(skill))
            .collect())
    }

    pub fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        self.connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?
            .query_row(
                "SELECT first_run_completed_at FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_catalog_error)
    }

    pub fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        self.connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?
            .execute(
                "UPDATE catalog_meta SET first_run_completed_at = ?1 WHERE singleton = 1",
                [unix_timestamp()],
            )
            .map_err(sqlite_catalog_error)?;
        Ok(())
    }

    /// Recently-enabled Skills for the tray quick view, newest enable first.
    /// Only Skills with at least one currently desired Activation appear
    /// (`desired_enabled = 1`); `last_enabled_at` is an epoch-seconds string,
    /// so it is compared as an integer; ties fall back to the stable
    /// directory-name order.
    pub fn recently_enabled_skills(
        &self,
        limit: u32,
    ) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        let connection = self
            .connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?;
        let mut statement = connection
            .prepare(
                "SELECT
                    skills.id, skills.directory_name, skills.display_name,
                    skills.description, skills.source_kind, skills.health,
                    COALESCE(SUM(CASE WHEN activations.desired_enabled = 1 THEN 1 ELSE 0 END), 0)
                 FROM skills
                 JOIN activations ON activations.skill_id = skills.id
                 WHERE activations.last_enabled_at IS NOT NULL
                   AND activations.desired_enabled = 1
                 GROUP BY skills.id
                 ORDER BY MAX(CAST(activations.last_enabled_at AS INTEGER)) DESC,
                          skills.directory_name COLLATE NOCASE, skills.id
                 LIMIT ?1",
            )
            .map_err(sqlite_catalog_error)?;
        statement
            .query_map([limit], |row| {
                Ok(SkillSummary {
                    id: SkillId(row.get(0)?),
                    directory_name: row.get(1)?,
                    display_name: row.get(2)?,
                    description: row.get(3)?,
                    source_kind: parse_source_kind(&row.get::<_, String>(4)?)?,
                    health: parse_health(&row.get::<_, String>(5)?)?,
                    enabled_agent_count: row.get(6)?,
                })
            })
            .map_err(sqlite_catalog_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_catalog_error)
    }

    pub fn persisted_skill_detail(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<PersistedSkillDetail>, CatalogStoreError> {
        self.connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?
            .query_row(
                "SELECT
                    skills.id, skills.directory_name, skills.display_name,
                    skills.description, skills.source_kind, skills.health,
                    skills.final_entity_path, skills.updated_at,
                    file_sources.original_path,
                    (SELECT COUNT(*) FROM activations
                     WHERE activations.skill_id = skills.id
                       AND activations.desired_enabled = 1)
                 FROM skills
                 LEFT JOIN file_sources ON file_sources.skill_id = skills.id
                 WHERE skills.id = ?1",
                [&skill_id.0],
                |row| {
                    Ok(PersistedSkillDetail {
                        summary: SkillSummary {
                            id: SkillId(row.get(0)?),
                            directory_name: row.get(1)?,
                            display_name: row.get(2)?,
                            description: row.get(3)?,
                            source_kind: parse_source_kind(&row.get::<_, String>(4)?)?,
                            health: parse_health(&row.get::<_, String>(5)?)?,
                            enabled_agent_count: row.get(9)?,
                        },
                        final_entity_path: PathBuf::from(row.get::<_, String>(6)?),
                        updated_at: row.get(7)?,
                        file_source_original_path: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_catalog_error)
    }

    pub fn list_agent_activations(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError> {
        let connection = self
            .connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?;
        let skill_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM skills WHERE id = ?1)",
                [&skill_id.0],
                |row| row.get(0),
            )
            .map_err(sqlite_catalog_error)?;
        if !skill_exists {
            return Ok(None);
        }
        let mut statement = connection
            .prepare(
                "SELECT
                    agents.id, agents.name, agents.kind, agents.skills_path,
                    agents.detected, agents.compatibility,
                    COALESCE(activations.desired_enabled, 0),
                    COALESCE(activations.observed_state, 'missing')
                 FROM agents
                 LEFT JOIN activations
                    ON activations.agent_id = agents.id AND activations.skill_id = ?1
                 ORDER BY agents.rowid",
            )
            .map_err(sqlite_catalog_error)?;
        statement
            .query_map([&skill_id.0], |row| {
                Ok(AgentActivation {
                    id: AgentId(row.get(0)?),
                    name: row.get(1)?,
                    kind: parse_agent_kind(&row.get::<_, String>(2)?)?,
                    skills_path: row.get(3)?,
                    detected: row.get(4)?,
                    compatibility: parse_compatibility(&row.get::<_, String>(5)?)?,
                    desired_enabled: row.get(6)?,
                    observed_state: parse_observed_state(&row.get::<_, String>(7)?)?,
                })
            })
            .map_err(sqlite_catalog_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map(Some)
            .map_err(sqlite_catalog_error)
    }

    pub fn adopted_skill_entities(&self) -> Result<Vec<AdoptedSkillEntity>, MaintenanceStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare("SELECT id, final_entity_path, recorded_content_hash FROM skills")
            .map_err(sqlite_maintenance_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(AdoptedSkillEntity {
                    skill_id: SkillId(row.get(0)?),
                    final_entity_path: PathBuf::from(row.get::<_, String>(1)?),
                    recorded_content_hash: row.get(2)?,
                })
            })
            .map_err(sqlite_maintenance_error)?;
        let mut entities = Vec::new();
        for row in rows {
            entities.push(row.map_err(sqlite_maintenance_error)?);
        }
        Ok(entities)
    }

    pub fn installed_skill_baselines(
        &self,
    ) -> Result<Vec<InstalledSkillBaseline>, MaintenanceStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare(
                "SELECT id, final_entity_path, recorded_content_hash
                 FROM skills
                 WHERE source_kind IN ('remote_install', 'file_install')
                   AND recorded_content_hash IS NOT NULL
                 ORDER BY id",
            )
            .map_err(sqlite_maintenance_error)?;
        statement
            .query_map([], |row| {
                Ok(InstalledSkillBaseline {
                    skill_id: SkillId(row.get(0)?),
                    final_entity_path: PathBuf::from(row.get::<_, String>(1)?),
                    recorded_content_hash: row.get(2)?,
                })
            })
            .map_err(sqlite_maintenance_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_maintenance_error)
    }

    pub fn record_skill_health(
        &self,
        observations: &[SkillHealthObservation],
    ) -> Result<u64, MaintenanceStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_maintenance_error)?;
        let mut changed = false;
        for observation in observations {
            changed |= transaction
                .execute(
                    "UPDATE skills
                     SET health = ?1,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?2 AND health != ?1",
                    params![health_value(observation.health), observation.skill_id.0],
                )
                .map_err(sqlite_maintenance_error)?
                > 0;
        }
        if changed {
            transaction
                .execute(
                    "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                    [],
                )
                .map_err(sqlite_maintenance_error)?;
        }
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_maintenance_error)?;
        transaction.commit().map_err(sqlite_maintenance_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            MaintenanceStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, ActivationStoreError> {
        self.connection
            .lock()
            .map_err(|_| ActivationStoreError::Unavailable("SQLite lock poisoned".into()))
    }

    /// Every Managed Skill row (Link and Install) with the inputs the health
    /// check recomputes Broken/Modified from.
    pub fn managed_skill_baselines(
        &self,
    ) -> Result<Vec<ManagedSkillBaseline>, MaintenanceStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare(
                "SELECT id, source_kind, final_entity_path, recorded_content_hash
                 FROM skills
                 ORDER BY id",
            )
            .map_err(sqlite_maintenance_error)?;
        statement
            .query_map([], |row| {
                Ok(ManagedSkillBaseline {
                    skill_id: SkillId(row.get(0)?),
                    source_kind: parse_source_kind(&row.get::<_, String>(1)?)?,
                    final_entity_path: PathBuf::from(row.get::<_, String>(2)?),
                    recorded_content_hash: row.get(3)?,
                })
            })
            .map_err(sqlite_maintenance_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_maintenance_error)
    }

    /// The persisted Link pointer; `None` for non-Link or unknown Skills.
    pub fn link_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<LinkSkillRecord>, MaintenanceStoreError> {
        self.connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT id, directory_name, display_name, description, final_entity_path
                 FROM skills
                 WHERE id = ?1 AND source_kind = 'link'",
                [&skill_id.0],
                |row| {
                    Ok(LinkSkillRecord {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        display_name: row.get(2)?,
                        description: row.get(3)?,
                        final_entity_path: PathBuf::from(row.get::<_, String>(4)?),
                    })
                },
            )
            .optional()
            .map_err(sqlite_maintenance_error)
    }

    /// Desired Activations of one Skill (only `desired_enabled = 1`, the
    /// records whose symlinks must be rewritten by a relocation).
    pub fn relocate_activations_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<RelocateActivationBaseline>, MaintenanceStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare(
                "SELECT agent_id, expected_entry_path, expected_target_path
                 FROM activations
                 WHERE skill_id = ?1 AND desired_enabled = 1
                 ORDER BY agent_id",
            )
            .map_err(sqlite_maintenance_error)?;
        statement
            .query_map([&skill_id.0], |row| {
                Ok(RelocateActivationBaseline {
                    agent_id: AgentId(row.get(0)?),
                    expected_entry_path: PathBuf::from(row.get::<_, String>(1)?),
                    expected_target_path: PathBuf::from(row.get::<_, String>(2)?),
                })
            })
            .map_err(sqlite_maintenance_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_maintenance_error)
    }

    /// Commit a relocation in one transaction: the Link pointer moves to the
    /// new entity, display metadata refreshes, health returns to Healthy,
    /// and every desired Activation repoints at the new target with observed
    /// state Present (the symlinks were already rewritten by the caller).
    pub fn commit_relocate(
        &self,
        skill_id: &SkillId,
        final_entity_path: PathBuf,
        display_name: String,
        description: String,
        new_target_path: PathBuf,
        activations: &[RelocateActivationBaseline],
    ) -> Result<u64, MaintenanceStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_maintenance_error)?;
        let updated_skills = transaction
            .execute(
                "UPDATE skills
                 SET final_entity_path = ?2, display_name = ?3, description = ?4,
                     health = 'healthy',
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?1 AND source_kind = 'link'",
                params![
                    skill_id.0,
                    final_entity_path.to_string_lossy(),
                    display_name,
                    description,
                ],
            )
            .map_err(sqlite_maintenance_error)?;
        if updated_skills == 0 {
            return Err(MaintenanceStoreError::Unavailable(format!(
                "the Link Skill '{}' disappeared before its relocation committed",
                skill_id.0
            )));
        }
        let checked_at = unix_timestamp();
        for activation in activations {
            transaction
                .execute(
                    "UPDATE activations
                     SET expected_target_path = ?3, observed_state = 'present',
                         last_checked_at = ?4
                     WHERE skill_id = ?1 AND agent_id = ?2 AND desired_enabled = 1",
                    params![
                        skill_id.0,
                        activation.agent_id.0,
                        new_target_path.to_string_lossy(),
                        checked_at,
                    ],
                )
                .map_err(sqlite_maintenance_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_maintenance_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_maintenance_error)?;
        transaction.commit().map_err(sqlite_maintenance_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            MaintenanceStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistedActivation {
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistedSkillDetail {
    pub summary: SkillSummary,
    pub final_entity_path: PathBuf,
    pub updated_at: String,
    pub file_source_original_path: Option<String>,
}

impl ImportStore for SqliteCatalogStore {
    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT id, directory_name FROM skills WHERE identity_key = ?1",
                [identity_key],
                |row| {
                    Ok(LibraryConflict {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_import_error)
    }

    fn insert_link(&self, record: LinkImportRecord) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT directory_name FROM skills WHERE identity_key = ?1",
                [&record.identity_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_import_error)?;
        if let Some(directory_name) = existing {
            return Err(ImportStoreError::Conflict(directory_name));
        }
        transaction
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, 'link', NULL, ?6, 'healthy',
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.skill_id.0,
                    record.directory_name,
                    record.identity_key,
                    record.display_name,
                    record.description,
                    record.final_entity_path.to_string_lossy(),
                ],
            )
            .map_err(sqlite_import_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn insert_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT directory_name FROM skills WHERE identity_key = ?1",
                [&record.identity_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_import_error)?;
        if let Some(directory_name) = existing {
            return Err(ImportStoreError::Conflict(directory_name));
        }
        transaction
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, 'file_install', ?6, ?7, ?8, 'healthy',
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.skill_id.0,
                    record.directory_name,
                    record.identity_key,
                    record.display_name,
                    record.description,
                    record.library_entry_path.to_string_lossy(),
                    record.final_entity_path.to_string_lossy(),
                    record.recorded_content_hash,
                ],
            )
            .map_err(sqlite_import_error)?;
        transaction
            .execute(
                "INSERT INTO file_sources (skill_id, original_path, original_filename, installed_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.skill_id.0,
                    record.original_path.to_string_lossy(),
                    record.original_filename,
                ],
            )
            .map_err(sqlite_import_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn insert_files(&self, records: Vec<FileImportRecord>) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        for record in records {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT directory_name FROM skills WHERE identity_key = ?1",
                    [&record.identity_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sqlite_import_error)?;
            if let Some(directory_name) = existing {
                return Err(ImportStoreError::Conflict(directory_name));
            }
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, 'file_install', ?6, ?7, ?8, 'healthy',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     )",
                    params![
                        record.skill_id.0,
                        record.directory_name,
                        record.identity_key,
                        record.display_name,
                        record.description,
                        record.library_entry_path.to_string_lossy(),
                        record.final_entity_path.to_string_lossy(),
                        record.recorded_content_hash,
                    ],
                )
                .map_err(sqlite_import_error)?;
            transaction
                .execute(
                    "INSERT INTO file_sources (
                        skill_id, original_path, original_filename, installed_at
                     ) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        record.skill_id.0,
                        record.original_path.to_string_lossy(),
                        record.original_filename,
                    ],
                )
                .map_err(sqlite_import_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn load_file_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<FileImportRecord>, ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT skills.id, skills.directory_name, skills.identity_key,
                        skills.display_name, skills.description,
                        skills.library_entry_path, skills.final_entity_path,
                        skills.recorded_content_hash, file_sources.original_path,
                        file_sources.original_filename
                   FROM skills
                   JOIN file_sources ON file_sources.skill_id = skills.id
                  WHERE skills.identity_key = ?1 AND skills.source_kind = 'file_install'",
                [identity_key],
                |row| {
                    Ok(FileImportRecord {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        identity_key: row.get(2)?,
                        display_name: row.get(3)?,
                        description: row.get(4)?,
                        library_entry_path: PathBuf::from(row.get::<_, String>(5)?),
                        final_entity_path: PathBuf::from(row.get::<_, String>(6)?),
                        recorded_content_hash: row.get(7)?,
                        original_path: PathBuf::from(row.get::<_, String>(8)?),
                        original_filename: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_import_error)
    }

    fn desired_activations_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<DesiredActivation>, ImportStoreError> {
        let mut activations = <Self as ActivationStore>::desired_activations(self)
            .map_err(|error| ImportStoreError::Unavailable(error.to_string()))?
            .into_iter()
            .filter(|activation| activation.skill_id == *skill_id)
            .collect::<Vec<_>>();
        activations.sort_by(|left, right| left.expected_entry_path.cmp(&right.expected_entry_path));
        Ok(activations)
    }

    fn replace_file(&self, record: FileImportRecord) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        let changed = transaction
            .execute(
                "UPDATE skills
                    SET display_name = ?2, description = ?3,
                        recorded_content_hash = ?4, health = 'healthy',
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE id = ?1 AND identity_key = ?5 AND source_kind = 'file_install'",
                params![
                    record.skill_id.0,
                    record.display_name,
                    record.description,
                    record.recorded_content_hash,
                    record.identity_key,
                ],
            )
            .map_err(sqlite_import_error)?;
        if changed != 1 {
            return Err(ImportStoreError::Unavailable(
                "file Install changed after reinstall preview".into(),
            ));
        }
        transaction
            .execute(
                "UPDATE file_sources
                    SET original_path = ?2, original_filename = ?3,
                        installed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE skill_id = ?1",
                params![
                    record.skill_id.0,
                    record.original_path.to_string_lossy(),
                    record.original_filename,
                ],
            )
            .map_err(sqlite_import_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn insert_remotes(&self, records: Vec<RemoteImportRecord>) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        for record in records {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT directory_name FROM skills WHERE identity_key = ?1",
                    [&record.identity_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sqlite_import_error)?;
            if let Some(directory_name) = existing {
                return Err(ImportStoreError::Conflict(directory_name));
            }
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?7, ?8, 'healthy',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     )",
                    params![
                        record.skill_id.0,
                        record.directory_name,
                        record.identity_key,
                        record.display_name,
                        record.description,
                        record.library_entry_path.to_string_lossy(),
                        record.final_entity_path.to_string_lossy(),
                        record.recorded_content_hash,
                    ],
                )
                .map_err(sqlite_import_error)?;
            transaction
                .execute(
                    "INSERT INTO remote_sources (
                        skill_id, source_url, requested_ref, resolved_commit, skill_path,
                        last_checked_at, last_updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5,
                        unixepoch('now'),
                        unixepoch('now')
                     )",
                    params![
                        record.skill_id.0,
                        record.source_url,
                        record.requested_ref,
                        record.resolved_commit,
                        record.skill_path,
                    ],
                )
                .map_err(sqlite_import_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn load_remote_installs(&self) -> Result<Vec<RemoteInstallRecord>, ImportStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare(
                "SELECT skills.id, skills.directory_name, skills.identity_key,
                        skills.display_name, skills.description,
                        skills.final_entity_path, skills.recorded_content_hash,
                        skills.health, remote_sources.source_url,
                        remote_sources.requested_ref, remote_sources.resolved_commit,
                        remote_sources.skill_path, remote_sources.last_checked_at,
                        remote_sources.last_updated_at
                   FROM skills
                   JOIN remote_sources ON remote_sources.skill_id = skills.id
                  WHERE skills.source_kind = 'remote_install'
                  ORDER BY skills.directory_name",
            )
            .map_err(sqlite_import_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(RemoteInstallRecord {
                    skill_id: SkillId(row.get(0)?),
                    directory_name: row.get(1)?,
                    identity_key: row.get(2)?,
                    display_name: row.get(3)?,
                    description: row.get(4)?,
                    final_entity_path: PathBuf::from(row.get::<_, String>(5)?),
                    recorded_content_hash: row.get(6)?,
                    health: parse_health(&row.get::<_, String>(7)?)?,
                    source_url: row.get(8)?,
                    requested_ref: row.get(9)?,
                    resolved_commit: row.get(10)?,
                    skill_path: row.get(11)?,
                    last_checked_at: row.get::<_, Option<i64>>(12)?,
                    last_updated_at: row.get::<_, Option<i64>>(13)?,
                })
            })
            .map_err(sqlite_import_error)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(sqlite_import_error)?);
        }
        Ok(records)
    }

    fn load_remote_install(
        &self,
        identity_key: &str,
    ) -> Result<Option<RemoteInstallRecord>, ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT skills.id, skills.directory_name, skills.identity_key,
                        skills.display_name, skills.description,
                        skills.final_entity_path, skills.recorded_content_hash,
                        skills.health, remote_sources.source_url,
                        remote_sources.requested_ref, remote_sources.resolved_commit,
                        remote_sources.skill_path, remote_sources.last_checked_at,
                        remote_sources.last_updated_at
                   FROM skills
                   JOIN remote_sources ON remote_sources.skill_id = skills.id
                  WHERE skills.identity_key = ?1 AND skills.source_kind = 'remote_install'",
                [identity_key],
                |row| {
                    Ok(RemoteInstallRecord {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        identity_key: row.get(2)?,
                        display_name: row.get(3)?,
                        description: row.get(4)?,
                        final_entity_path: PathBuf::from(row.get::<_, String>(5)?),
                        recorded_content_hash: row.get(6)?,
                        health: parse_health(&row.get::<_, String>(7)?)?,
                        source_url: row.get(8)?,
                        requested_ref: row.get(9)?,
                        resolved_commit: row.get(10)?,
                        skill_path: row.get(11)?,
                        last_checked_at: row.get::<_, Option<i64>>(12)?,
                        last_updated_at: row.get::<_, Option<i64>>(13)?,
                    })
                },
            )
            .optional()
            .map_err(sqlite_import_error)
    }

    fn update_remote_install(&self, record: RemoteImportRecord) -> Result<u64, ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        let changed = transaction
            .execute(
                "UPDATE skills
                    SET display_name = ?2, description = ?3,
                        recorded_content_hash = ?4, health = 'healthy',
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE id = ?1 AND identity_key = ?5 AND source_kind = 'remote_install'",
                params![
                    record.skill_id.0,
                    record.display_name,
                    record.description,
                    record.recorded_content_hash,
                    record.identity_key,
                ],
            )
            .map_err(sqlite_import_error)?;
        if changed != 1 {
            return Err(ImportStoreError::Unavailable(
                "remote Install changed after Update preview".into(),
            ));
        }
        transaction
            .execute(
                "UPDATE remote_sources
                    SET resolved_commit = ?2, skill_path = ?3,
                        last_updated_at = unixepoch('now'),
                        last_checked_at = unixepoch('now')
                  WHERE skill_id = ?1",
                params![record.skill_id.0, record.resolved_commit, record.skill_path],
            )
            .map_err(sqlite_import_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| ImportStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                "UPDATE remote_sources SET last_checked_at = unixepoch('now') WHERE skill_id = ?1",
                [skill_id.0.as_str()],
            )
            .map_err(sqlite_import_error)?;
        Ok(())
    }

    fn set_remote_requested_ref(
        &self,
        skill_id: &SkillId,
        requested_ref: &str,
    ) -> Result<(), ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                "UPDATE remote_sources SET requested_ref = ?2 WHERE skill_id = ?1",
                params![skill_id.0, requested_ref],
            )
            .map_err(sqlite_import_error)?;
        Ok(())
    }
}

impl AdoptStore for SqliteCatalogStore {
    fn mark_agent_detected(&self, agent_id: &AgentId) -> Result<(), AdoptStoreError> {
        self.connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                "UPDATE agents SET detected = 1, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![unix_timestamp(), agent_id.0],
            )
            .map_err(sqlite_adopt_error)?;
        Ok(())
    }

    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        self.connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?
            .prepare("SELECT id, name, kind, skills_path, detected FROM agents ORDER BY name")
            .map_err(sqlite_adopt_error)?
            .query_map([], |row| {
                Ok(AdoptAgent {
                    agent_id: AgentId(row.get(0)?),
                    name: row.get(1)?,
                    kind: parse_agent_kind(&row.get::<_, String>(2)?)?,
                    skills_path: PathBuf::from(row.get::<_, String>(3)?),
                    detected: row.get::<_, bool>(4)?,
                })
            })
            .map_err(sqlite_adopt_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_adopt_error)
    }

    fn insert_adopted(&self, record: AdoptedSkillRecord) -> Result<u64, AdoptStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_adopt_error)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT directory_name FROM skills WHERE identity_key = ?1",
                [&record.identity_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_adopt_error)?;
        if let Some(directory_name) = existing {
            return Err(AdoptStoreError::Conflict(directory_name));
        }
        let source_kind = if record.library_entry_path.is_some() {
            "file_install"
        } else {
            "link"
        };
        transaction
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'healthy',
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.skill_id.0,
                    record.directory_name,
                    record.identity_key,
                    record.display_name,
                    record.description,
                    source_kind,
                    record
                        .library_entry_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    record.final_entity_path.to_string_lossy(),
                    record.recorded_content_hash,
                ],
            )
            .map_err(sqlite_adopt_error)?;
        if let (Some(original_path), Some(_)) =
            (&record.original_path, &record.recorded_content_hash)
        {
            transaction
                .execute(
                    "INSERT INTO file_sources (skill_id, original_path, original_filename, installed_at)
                     VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![record.skill_id.0, original_path.to_string_lossy(), record.original_filename],
                )
                .map_err(sqlite_adopt_error)?;
        }
        for activation in &record.activations {
            transaction
                .execute(
                    "INSERT INTO activations (
                        skill_id, agent_id, desired_enabled, expected_entry_path,
                        expected_target_path, observed_state, last_enabled_at, last_checked_at
                     ) VALUES (?1, ?2, 1, ?3, ?4, 'present',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        record.skill_id.0,
                        activation.agent_id.0,
                        activation.expected_entry_path.to_string_lossy(),
                        activation.expected_target_path.to_string_lossy(),
                    ],
                )
                .map_err(sqlite_adopt_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_adopt_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_adopt_error)?;
        transaction.commit().map_err(sqlite_adopt_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| AdoptStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn remove_adopted_skill(&self, skill_id: &SkillId) -> Result<u64, AdoptStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_adopt_error)?;
        let changed = transaction
            .execute("DELETE FROM skills WHERE id = ?1", [skill_id.0.as_str()])
            .map_err(sqlite_adopt_error)?;
        if changed != 1 {
            return Err(AdoptStoreError::Unavailable(
                "the adopted Skill is no longer present".into(),
            ));
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_adopt_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(sqlite_adopt_error)?;
        transaction.commit().map_err(sqlite_adopt_error)?;
        u64::try_from(snapshot_version)
            .map_err(|_| AdoptStoreError::Unavailable("negative SQLite snapshot version".into()))
    }

    fn find_library_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<AdoptLibraryConflict>, AdoptStoreError> {
        self.connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT id, directory_name, final_entity_path FROM skills WHERE identity_key = ?1",
                [identity_key],
                |row| {
                    Ok(AdoptLibraryConflict {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        final_entity_path: PathBuf::from(row.get::<_, String>(2)?),
                    })
                },
            )
            .optional()
            .map_err(sqlite_adopt_error)
    }
}

impl crate::seams::preferences_store::PreferencesStore for SqliteCatalogStore {
    fn load_preferences(
        &self,
    ) -> Result<crate::seams::preferences_store::AppPreferences, PreferencesStoreError> {
        let connection = self
            .connection()
            .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
        connection
            .query_row(
                "SELECT launch_at_login, show_in_dock, check_app_updates, check_skill_updates
                 FROM preferences WHERE singleton = 1",
                [],
                |row| {
                    Ok(crate::seams::preferences_store::AppPreferences {
                        launch_at_login: row.get(0)?,
                        show_in_dock: row.get(1)?,
                        check_app_updates: row.get(2)?,
                        check_skill_updates: row.get(3)?,
                    })
                },
            )
            .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))
    }

    fn update_preferences(
        &self,
        updates: crate::seams::preferences_store::PreferenceUpdates,
    ) -> Result<crate::seams::preferences_store::AppPreferences, PreferencesStoreError> {
        {
            let mut connection = self
                .connection()
                .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            if let Some(value) = updates.launch_at_login {
                transaction
                    .execute(
                        "UPDATE preferences SET launch_at_login = ?1 WHERE singleton = 1",
                        [value],
                    )
                    .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            }
            if let Some(value) = updates.show_in_dock {
                transaction
                    .execute(
                        "UPDATE preferences SET show_in_dock = ?1 WHERE singleton = 1",
                        [value],
                    )
                    .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            }
            if let Some(value) = updates.check_app_updates {
                transaction
                    .execute(
                        "UPDATE preferences SET check_app_updates = ?1 WHERE singleton = 1",
                        [value],
                    )
                    .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            }
            if let Some(value) = updates.check_skill_updates {
                transaction
                    .execute(
                        "UPDATE preferences SET check_skill_updates = ?1 WHERE singleton = 1",
                        [value],
                    )
                    .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
            }
            transaction
                .commit()
                .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
        }
        // The connection guard is released before reloading, or the reload
        // would deadlock on the same mutex.
        Self::load_preferences(self)
    }
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
    if existing_schema_version == 1 {
        transaction.execute_batch(
            "CREATE TABLE file_sources (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                original_path TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                installed_at TEXT NOT NULL
             );
             UPDATE catalog_meta SET schema_version = 2 WHERE singleton = 1;",
        )?;
    }
    if existing_schema_version == 2 {
        transaction.execute_batch(
            "CREATE TABLE remote_sources (
                skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
                source_url TEXT NOT NULL,
                requested_ref TEXT NOT NULL,
                resolved_commit TEXT NOT NULL,
                skill_path TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
             );
             UPDATE catalog_meta SET schema_version = 3 WHERE singleton = 1;",
        )?;
    }
    if existing_schema_version == 3 {
        transaction.execute_batch(
            "ALTER TABLE catalog_meta ADD COLUMN first_run_completed_at TEXT;
             UPDATE catalog_meta SET schema_version = 4 WHERE singleton = 1;",
        )?;
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

fn sqlite_catalog_error(error: rusqlite::Error) -> CatalogStoreError {
    CatalogStoreError::Unavailable(error.to_string())
}

fn sqlite_import_error(error: rusqlite::Error) -> ImportStoreError {
    ImportStoreError::Unavailable(error.to_string())
}

fn sqlite_maintenance_error(error: rusqlite::Error) -> MaintenanceStoreError {
    MaintenanceStoreError::Unavailable(error.to_string())
}

fn source_kind_value(value: SourceKind) -> &'static str {
    match value {
        SourceKind::Link => "link",
        SourceKind::RemoteInstall => "remote_install",
        SourceKind::FileInstall => "file_install",
    }
}

fn parse_source_kind(value: &str) -> rusqlite::Result<SourceKind> {
    match value {
        "link" => Ok(SourceKind::Link),
        "remote_install" => Ok(SourceKind::RemoteInstall),
        "file_install" => Ok(SourceKind::FileInstall),
        value => Err(invalid_enum_value(4, value)),
    }
}

fn health_value(value: Health) -> &'static str {
    match value {
        Health::Healthy => "healthy",
        Health::Broken => "broken",
        Health::Modified => "modified",
    }
}

fn parse_health(value: &str) -> rusqlite::Result<Health> {
    match value {
        "healthy" => Ok(Health::Healthy),
        "broken" => Ok(Health::Broken),
        "modified" => Ok(Health::Modified),
        value => Err(invalid_enum_value(5, value)),
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

fn parse_compatibility(value: &str) -> rusqlite::Result<Compatibility> {
    match value {
        "verified" => Ok(Compatibility::Verified),
        "unknown" => Ok(Compatibility::Unknown),
        value => Err(invalid_enum_value(5, value)),
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

fn sqlite_adopt_error(error: rusqlite::Error) -> AdoptStoreError {
    AdoptStoreError::Unavailable(error.to_string())
}
