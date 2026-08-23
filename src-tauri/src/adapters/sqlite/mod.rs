use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentActivation, AgentId, AgentKind, CatalogFilter, Compatibility,
    Health, SkillId, SkillSummary, SourceKind,
};
use crate::core::home::{BoundHome, HomeId};
use crate::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedSkillRecord,
    LibraryConflict as AdoptLibraryConflict, RemoteAdoptedSkillRecord,
};
use crate::seams::catalog_probe::{CURRENT_CATALOG_SCHEMA_VERSION, CatalogHomeIdentity};
use crate::seams::catalog_store::{
    CatalogStoreError, StartupAccess, StartupDiagnostic, StartupDiagnosticCode, StartupStatus,
};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict, LinkImportRecord,
    RemoteImportRecord, RemoteInstallRecord, RemoteParentRecord,
};
use crate::seams::legacy_migration::{LegacyCatalogMigrator, LegacyMigrationError};
use crate::seams::maintenance_store::{
    AdoptedSkillEntity, HandoffRecoveredRecord, InstalledSkillBaseline, LinkSkillRecord,
    MaintenanceStoreError, ManagedSkillBaseline, RelocateActivationBaseline, RemoveTarget,
    SkillHealthObservation,
};
use crate::seams::preferences_store::PreferencesStoreError;
use crate::seams::source_promotion_store::{
    LegacySourcePromotionMemberRecord, LegacySourcePromotionRecord,
    SourcePromotionActivationRecord, SourcePromotionMemberOrigin, SourcePromotionRecord,
    SourcePromotionRemovedMemberRecord, SourcePromotionStore, SourcePromotionStoreError,
};
use crate::seams::source_transition_store::{
    SourceTransitionRecord, SourceTransitionStore, SourceTransitionStoreError,
};

mod prepared;
pub use prepared::SqlitePreparedCatalogFactory;

pub const CURRENT_SCHEMA_VERSION: u32 = CURRENT_CATALOG_SCHEMA_VERSION;

/// System `LegacyCatalogMigrator`: wraps the one-time Catalog transition
/// owned by the Home Binding flow. Never called by ordinary `open()`, which
/// probes pre-identity schemas read-only (spec §3.4).
pub struct SqliteLegacyCatalogMigrator;

impl LegacyCatalogMigrator for SqliteLegacyCatalogMigrator {
    fn checkpoint(&self, path: &Path) -> Result<(), LegacyMigrationError> {
        SqliteCatalogStore::checkpoint_wal(path)
    }

    fn migrate_with_identity(
        &self,
        path: &Path,
        identity: &CatalogHomeIdentity,
    ) -> Result<PathBuf, LegacyMigrationError> {
        SqliteCatalogStore::migrate_with_identity(path, identity)
    }
}

const INITIAL_SCHEMA: &str = r#"
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
    original_commit_known INTEGER NOT NULL DEFAULT 0 CHECK (original_commit_known IN (0, 1)),
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
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    launch_at_login INTEGER NOT NULL DEFAULT 0 CHECK (launch_at_login IN (0, 1)),
    show_in_dock INTEGER NOT NULL DEFAULT 1 CHECK (show_in_dock IN (0, 1)),
    check_app_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_app_updates IN (0, 1)),
    check_skill_updates INTEGER NOT NULL DEFAULT 1 CHECK (check_skill_updates IN (0, 1)),
    last_app_update_check_at TEXT,
    last_skill_update_check_at TEXT
);

INSERT INTO catalog_meta (singleton, schema_version, snapshot_version)
VALUES (1, 7, 0);

INSERT INTO preferences (singleton) VALUES (1);
"#;

#[derive(Debug, Error)]
pub enum CatalogStoreOpenError {
    #[error("could not open the SQLite catalog: {0}")]
    Open(#[source] rusqlite::Error),
    #[error("could not configure the SQLite catalog: {0}")]
    Configure(#[source] rusqlite::Error),
    #[error("could not back up the SQLite catalog: {0}")]
    Backup(#[source] rusqlite::Error),
}

/// Errors opening or creating the Catalog of a verified `BoundHome`.
#[derive(Debug, Error)]
pub enum BoundCatalogOpenError {
    #[error("the Catalog does not exist at the bound Home path")]
    Missing,
    #[error("the Catalog schema {found} is not the bound schema {CURRENT_SCHEMA_VERSION}")]
    SchemaNotBound { found: u32 },
    #[error("the Catalog has no recorded Home identity")]
    IdentityMissing,
    #[error("the Catalog identity does not match the bound Home")]
    IdentityMismatch,
    #[error("could not open the bound SQLite catalog: {0}")]
    Open(#[source] rusqlite::Error),
    #[error("could not configure the bound SQLite catalog: {0}")]
    Configure(#[source] CatalogStoreOpenError),
    #[error("could not migrate the fresh Catalog: {0}")]
    Migration(#[source] rusqlite::Error),
    #[error("could not write the Home identity into the Catalog: {0}")]
    WriteIdentity(#[source] rusqlite::Error),
    #[error("a Catalog already exists at the candidate path")]
    AlreadyExists,
}

pub struct SqliteCatalogStore {
    connection: Mutex<Connection>,
    startup_status: StartupStatus,
}

impl SqliteCatalogStore {
    /// Test/binding-internal convenience: opens an existing Catalog, creating
    /// a fresh current-schema file only when the path does not exist. Production
    /// composition never calls this without a verified `BoundHome`; the
    /// bootstrap authority resolves state read-only first (spec §3.4: a
    /// pre-identity Catalog is only probed, never auto-migrated).
    pub fn open(path: &Path) -> Result<Self, CatalogStoreOpenError> {
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

        // Pre-identity schemas (v1–v4) may only be probed read-only: the
        // one-time Legacy transition owns their migration (spec §3.4).
        // Identity-bearing v5+ Catalogs migrate in place to the current schema below.
        if existing_schema_version != 0 && existing_schema_version < 5 {
            let connection = open_read_only(path)?;
            return Ok(Self {
                connection: Mutex::new(connection),
                startup_status: StartupStatus {
                    access: StartupAccess::ReadOnly,
                    schema_version: existing_schema_version,
                    diagnostic: Some(StartupDiagnostic {
                        code: StartupDiagnosticCode::MigrationRequired,
                        message: format!(
                            "Catalog schema {existing_schema_version} predates Home identity; migration is owned by the Home Binding/Legacy flow."
                        ),
                        backup_path: None,
                    }),
                },
            });
        }

        let mut connection = Connection::open(path).map_err(CatalogStoreOpenError::Open)?;
        configure_connection(&connection)?;

        if existing_schema_version == 0
            || (5..CURRENT_SCHEMA_VERSION).contains(&existing_schema_version)
        {
            // Fresh file: create the current schema. Identity-bearing v5+:
            // migrate in place to the current schema (spec §3.4). Ordinary open() never
            // migrates pre-identity data.
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
                            backup_path: None,
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

    /// Read-only open for a verified Bound Home whose writable open failed
    /// (permission, lock, transient I/O): the session continues read-only
    /// with the gate in `CatalogReadOnly`. Never migrates or writes.
    pub fn open_read_only(path: &Path) -> Result<Self, CatalogStoreOpenError> {
        let connection = open_read_only(path)?;
        let schema_version = detect_schema_version(path);
        Ok(Self {
            connection: Mutex::new(connection),
            startup_status: StartupStatus {
                access: StartupAccess::ReadOnly,
                schema_version,
                diagnostic: None,
            },
        })
    }

    /// Open the Catalog of a verified `BoundHome` for writing. Re-verifies
    /// the recorded identity against the bound value object (spec §3.4:
    /// `BoundCatalogStore` construction requires the four identity fields to
    /// be present and matching). An identity-bearing v5 Catalog migrates in
    /// place to the current schema; anything else is refused.
    pub fn open_bound(bound: &BoundHome, path: &Path) -> Result<Self, BoundCatalogOpenError> {
        if !has_content(path) {
            return Err(BoundCatalogOpenError::Missing);
        }
        let schema = detect_schema_version(path);
        if !(5..=CURRENT_SCHEMA_VERSION).contains(&schema) {
            return Err(BoundCatalogOpenError::SchemaNotBound { found: schema });
        }
        let mut connection = Connection::open(path).map_err(BoundCatalogOpenError::Open)?;
        configure_connection(&connection).map_err(BoundCatalogOpenError::Configure)?;
        let stored =
            read_catalog_identity(&connection).ok_or(BoundCatalogOpenError::IdentityMissing)?;
        if stored.home_id != bound.home_id
            || stored.volume_fsid != bound.volume_fsid
            || stored.volume_uuid != bound.volume_uuid
        {
            return Err(BoundCatalogOpenError::IdentityMismatch);
        }
        if schema < CURRENT_SCHEMA_VERSION {
            migrate_to_current(&mut connection, schema)
                .map_err(BoundCatalogOpenError::Migration)?;
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

    /// Create a fresh current Catalog carrying `bound`'s identity. The primitive
    /// behind the Home Binding flow (ticket #45) and the test Home helper;
    /// refuses an existing file so it can never overwrite real data.
    pub fn create_bound(bound: &BoundHome, path: &Path) -> Result<Self, BoundCatalogOpenError> {
        if has_content(path) {
            return Err(BoundCatalogOpenError::AlreadyExists);
        }
        let mut connection = Connection::open(path).map_err(BoundCatalogOpenError::Open)?;
        configure_connection(&connection).map_err(BoundCatalogOpenError::Configure)?;
        migrate_to_current(&mut connection, 0).map_err(BoundCatalogOpenError::Migration)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(BoundCatalogOpenError::WriteIdentity)?;
        transaction
            .execute(
                "UPDATE catalog_meta
                 SET home_id = ?1, volume_fsid = ?2, volume_uuid = ?3, home_bound_at = ?4
                 WHERE singleton = 1",
                params![
                    bound.home_id.0,
                    bound.volume_fsid,
                    bound.volume_uuid,
                    bound.bound_at,
                ],
            )
            .map_err(BoundCatalogOpenError::WriteIdentity)?;
        transaction
            .commit()
            .map_err(BoundCatalogOpenError::WriteIdentity)?;
        let stored =
            read_catalog_identity(&connection).ok_or(BoundCatalogOpenError::IdentityMissing)?;
        if stored.home_id != bound.home_id
            || stored.volume_fsid != bound.volume_fsid
            || stored.volume_uuid != bound.volume_uuid
        {
            return Err(BoundCatalogOpenError::IdentityMismatch);
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

    /// Checkpoint the Catalog WAL into the main database (TRUNCATE) so a
    /// later tree copy carries the consistent SQLite+WAL+SHM set. The
    /// Legacy copy transition calls this after proving no other process
    /// holds the WAL index.
    pub fn checkpoint_wal(path: &Path) -> Result<(), LegacyMigrationError> {
        let connection = Connection::open(path)
            .map_err(|error| LegacyMigrationError::Open(error.to_string()))?;
        configure_connection(&connection)
            .map_err(|error| LegacyMigrationError::Open(error.to_string()))?;
        connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .map_err(|error| LegacyMigrationError::Checkpoint(error.to_string()))?;
        Ok(())
    }

    /// One-time Home Binding / Legacy transition (spec §3.4, §5.4): migrates
    /// a pre-identity Catalog (v1–v4) to the current schema, records the
    /// Home identity and verifies it. The untouched Catalog is backed up to
    /// a sibling `pre-migration-v<schema>.bak` file first; the caller owns
    /// the crash-recovery contract around the locator commit. Never called
    /// by ordinary `open()`, which probes pre-identity schemas read-only.
    pub fn migrate_with_identity(
        path: &Path,
        identity: &CatalogHomeIdentity,
    ) -> Result<PathBuf, LegacyMigrationError> {
        let schema = detect_schema_version(path);
        if schema == 0 || schema > CURRENT_SCHEMA_VERSION {
            return Err(LegacyMigrationError::NotPreIdentity { found: schema });
        }
        // Pre-identity schemas are backed up untouched; a current-schema
        // Catalog (e.g. a fixture-recovery prepared Home or a retried
        // transition) is migrated in place idempotently.
        let backup_path = if schema < CURRENT_SCHEMA_VERSION {
            Some(
                backup_catalog(path, schema)
                    .map_err(|error| LegacyMigrationError::Backup(error.to_string()))?,
            )
        } else {
            None
        };
        let mut connection = Connection::open(path)
            .map_err(|error| LegacyMigrationError::Open(error.to_string()))?;
        configure_connection(&connection)
            .map_err(|error| LegacyMigrationError::Open(error.to_string()))?;
        // Quiesce the WAL into the main database so the copied consistent
        // set (SQLite + WAL + SHM) carries every committed row.
        connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .map_err(|error| LegacyMigrationError::Checkpoint(error.to_string()))?;
        if schema < CURRENT_SCHEMA_VERSION {
            migrate_to_current(&mut connection, schema)
                .map_err(|error| LegacyMigrationError::Migration(error.to_string()))?;
        }
        // Never overwrite a different recorded identity: a mismatched
        // Catalog is a closed error, not a repair.
        if let Some(existing) = read_catalog_identity(&connection) {
            if existing != *identity {
                return Err(LegacyMigrationError::Identity(
                    "recorded identity differs from the requested binding".into(),
                ));
            }
        } else {
            connection
                .execute(
                    "UPDATE catalog_meta
                     SET home_id = ?1, volume_fsid = ?2, volume_uuid = ?3, home_bound_at = ?4
                     WHERE singleton = 1",
                    params![
                        identity.home_id.0,
                        identity.volume_fsid,
                        identity.volume_uuid,
                        identity.home_bound_at
                    ],
                )
                .map_err(|error| LegacyMigrationError::Identity(error.to_string()))?;
        }
        let stored = read_catalog_identity(&connection)
            .ok_or_else(|| LegacyMigrationError::Identity("missing".into()))?;
        if stored != *identity {
            return Err(LegacyMigrationError::Identity(
                "recorded identity differs from the requested binding".into(),
            ));
        }
        Ok(backup_path.unwrap_or_else(|| path.to_path_buf()))
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
    pub fn activation_baselines_for_skill(
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

    /// The persisted identity of a Managed Skill targeted by Remove.
    pub fn remove_target(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<RemoveTarget>, MaintenanceStoreError> {
        self.connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT id, directory_name, source_kind, final_entity_path
                 FROM skills
                 WHERE id = ?1",
                [&skill_id.0],
                |row| {
                    Ok(RemoveTarget {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        source_kind: parse_source_kind(&row.get::<_, String>(2)?)?,
                        final_entity_path: PathBuf::from(row.get::<_, String>(3)?),
                    })
                },
            )
            .optional()
            .map_err(sqlite_maintenance_error)
    }

    /// Delete the Skill row; activations, file_sources and remote_sources
    /// cascade with the row (§5.4), and the operation audit lives in the
    /// archived Remove journal instead.
    pub fn delete_skill(&self, skill_id: &SkillId) -> Result<u64, MaintenanceStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_maintenance_error)?;
        let deleted = transaction
            .execute("DELETE FROM skills WHERE id = ?1", [&skill_id.0])
            .map_err(sqlite_maintenance_error)?;
        if deleted == 0 {
            return Err(MaintenanceStoreError::Unavailable(format!(
                "the Managed Skill '{}' disappeared before its Remove committed",
                skill_id.0
            )));
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

    /// The Binding's parent id of a managed remote Install; `None` when
    /// the skill has no binding (captured before Remove deletes the row).
    pub fn binding_remote_id(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, MaintenanceStoreError> {
        self.connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT remote_id FROM remote_bindings WHERE skill_id = ?1",
                [skill_id.0.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_maintenance_error)
    }

    /// Delete the parent row when it has no remaining child bindings;
    /// returns whether the parent was deleted (last-child Remove).
    pub fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, MaintenanceStoreError> {
        self.remove_parent_if_last_child(remote_id)
            .map_err(MaintenanceStoreError::Unavailable)
    }

    /// Crash roll-forward of a committed Handoff (spec §8.4 step 5): the
    /// parent upsert, Skill row, Binding and Activations commit in one
    /// idempotent transaction — existing rows are left untouched.
    pub fn insert_handoff_recovered(
        &self,
        record: HandoffRecoveredRecord,
    ) -> Result<u64, MaintenanceStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_maintenance_error)?;
        let existing: Option<String> = transaction
            .query_row(
                "SELECT directory_name FROM skills WHERE id = ?1",
                [&record.skill.skill_id.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_maintenance_error)?;
        if existing.is_none() {
            // Crash roll-forward journals may not carry a resolved parent id
            // (the crash happened before the remote_id was assigned); a
            // fresh UUID is safe because the canonical-URL conflict clause
            // reuses any existing parent.
            let remote_id = if record.skill.remote_id.is_empty() {
                new_remote_id(&transaction).map_err(sqlite_maintenance_error)?
            } else {
                record.skill.remote_id.clone()
            };
            transaction
                .execute(
                    "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                     VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ON CONFLICT(canonical_url) DO NOTHING",
                    params![remote_id, record.skill.source_url],
                )
                .map_err(sqlite_maintenance_error)?;
            let remote_id: String = transaction
                .query_row(
                    "SELECT remote_id FROM remote_source_parents WHERE canonical_url = ?1",
                    [&record.skill.source_url],
                    |row| row.get(0),
                )
                .map_err(sqlite_maintenance_error)?;
            let health = if record.skill.remote_baseline_hash == record.skill.current_baseline_hash
            {
                "healthy"
            } else {
                "modified"
            };
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?6, ?7, ?8,
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     )",
                    params![
                        record.skill.skill_id.0,
                        record.skill.directory_name,
                        record.skill.identity_key,
                        record.skill.display_name,
                        record.skill.description,
                        record.skill.final_entity_path.to_string_lossy(),
                        record.skill.current_baseline_hash,
                        health,
                    ],
                )
                .map_err(sqlite_maintenance_error)?;
            transaction
                .execute(
                    "INSERT INTO remote_bindings (
                        skill_id, remote_id, requested_ref, verification_anchor_commit,
                        original_commit_known, skill_path, provider_hash,
                        remote_baseline_hash, current_baseline_hash,
                        last_checked_at, last_updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                        unixepoch('now'), unixepoch('now')
                     )",
                    params![
                        record.skill.skill_id.0,
                        remote_id,
                        record.skill.requested_ref,
                        record.skill.verification_anchor_commit,
                        record.skill.original_commit_known as i64,
                        record.skill.skill_path,
                        record.skill.provider_hash,
                        record.skill.remote_baseline_hash,
                        record.skill.current_baseline_hash,
                    ],
                )
                .map_err(sqlite_maintenance_error)?;
            for activation in &record.activations {
                transaction
                    .execute(
                        "INSERT INTO activations (
                            skill_id, agent_id, desired_enabled, expected_entry_path,
                            expected_target_path, observed_state, last_enabled_at, last_checked_at
                         ) VALUES (?1, ?2, 1, ?3, ?4, 'present',
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                         ON CONFLICT(skill_id, agent_id) DO NOTHING",
                        params![
                            record.skill.skill_id.0,
                            activation.agent_id,
                            activation.expected_entry_path.to_string_lossy(),
                            activation.expected_target_path.to_string_lossy(),
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

    /// Shared parent lookup behind every store facade (Import/Adopt/
    /// Maintenance): one implementation, typed error wrappers per trait.
    fn remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "SQLite lock poisoned".to_string())?;
        let parent: Option<(String, String, String)> = connection
            .query_row(
                "SELECT remote_id, canonical_url, created_at
                   FROM remote_source_parents
                  WHERE canonical_url = ?1
                     OR remote_id IN (
                            SELECT remote_id FROM remote_source_aliases WHERE alias_url = ?1
                        )",
                [canonical_url],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some((remote_id, parent_url, created_at)) = parent else {
            return Ok(None);
        };
        let aliases = connection
            .prepare(
                "SELECT alias_url FROM remote_source_aliases WHERE remote_id = ?1 ORDER BY alias_url",
            )
            .map_err(|error| error.to_string())?
            .query_map([&remote_id], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(Some(RemoteParentRecord {
            remote_id,
            canonical_url: parent_url,
            created_at,
            aliases,
        }))
    }

    /// Shared last-child parent removal behind every store facade.
    fn remove_parent_if_last_child(&self, remote_id: &str) -> Result<bool, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "SQLite lock poisoned".to_string())?;
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM remote_bindings WHERE remote_id = ?1",
                [remote_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if remaining != 0 {
            return Ok(false);
        }
        let changed = connection
            .execute(
                "DELETE FROM remote_source_parents WHERE remote_id = ?1",
                [remote_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(changed == 1)
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

impl SourcePromotionStore for SqliteCatalogStore {
    fn read_legacy_source_promotion(
        &self,
        remote_id: &str,
    ) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        if integrity != "ok" {
            return Err(SourcePromotionStoreError::Conflict(
                "the Catalog integrity check is not clean".into(),
            ));
        }
        let mut foreign_key_check = connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        if foreign_key_check
            .query([])
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .next()
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .is_some()
        {
            return Err(SourcePromotionStoreError::Conflict(
                "the Catalog foreign-key check is not clean".into(),
            ));
        }
        let (canonical_url, created_at): (String, String) = connection
            .query_row(
                "SELECT canonical_url, created_at FROM remote_source_parents WHERE remote_id = ?1",
                [remote_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .ok_or_else(|| {
                SourcePromotionStoreError::Conflict("the selected parent no longer exists".into())
            })?;
        let already_promoted: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM git_repository_sources WHERE remote_id = ?1)",
                [remote_id],
                |row| row.get(0),
            )
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        if already_promoted {
            return Err(SourcePromotionStoreError::Conflict(
                "the selected parent is already a Git Repository Source".into(),
            ));
        }
        let aliases = connection
            .prepare(
                "SELECT alias_url FROM remote_source_aliases
                 WHERE remote_id = ?1 ORDER BY alias_url",
            )
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .query_map([remote_id], |row| row.get::<_, String>(0))
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;

        let mut statement = connection
            .prepare(
                "SELECT skills.id, skills.directory_name, skills.identity_key,
                        skills.display_name, skills.description, skills.library_entry_path,
                        skills.final_entity_path, skills.recorded_content_hash, skills.health,
                        remote_bindings.skill_path, remote_bindings.current_baseline_hash,
                        remote_bindings.requested_ref,
                        remote_bindings.verification_anchor_commit,
                        remote_bindings.original_commit_known, remote_bindings.provider_hash,
                        remote_bindings.remote_baseline_hash,
                        remote_bindings.last_checked_at, remote_bindings.last_updated_at,
                        skills.source_kind
                 FROM remote_bindings
                 JOIN skills ON skills.id = remote_bindings.skill_id
                 WHERE remote_bindings.remote_id = ?1
                 ORDER BY remote_bindings.skill_path, skills.id",
            )
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        let rows = statement
            .query_map([remote_id], |row| {
                Ok((
                    LegacySourcePromotionMemberRecord {
                        skill_id: SkillId(row.get(0)?),
                        directory_name: row.get(1)?,
                        identity_key: row.get(2)?,
                        display_name: row.get(3)?,
                        description: row.get(4)?,
                        library_entry_path: PathBuf::from(row.get::<_, String>(5)?),
                        final_entity_path: PathBuf::from(row.get::<_, String>(6)?),
                        recorded_content_hash: row.get(7)?,
                        health: parse_health(&row.get::<_, String>(8)?)?,
                        skill_path: row.get(9)?,
                        current_baseline_hash: row.get(10)?,
                        requested_ref: row.get(11)?,
                        verification_anchor_commit: row.get(12)?,
                        original_commit_known: row.get(13)?,
                        provider_hash: row.get(14)?,
                        remote_baseline_hash: row.get(15)?,
                        last_checked_at: row.get(16)?,
                        last_updated_at: row.get(17)?,
                        activations: Vec::new(),
                    },
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(18)?,
                ))
            })
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        if rows.is_empty() {
            return Err(SourcePromotionStoreError::Conflict(
                "the selected parent has no Legacy Per-Skill Git State members".into(),
            ));
        }
        let refs = rows
            .iter()
            .map(|(_, requested_ref, _)| requested_ref.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if refs.len() != 1 || refs.first().is_none_or(|value| value.is_empty()) {
            return Err(SourcePromotionStoreError::Conflict(
                "the selected parent does not have one non-empty tracking ref".into(),
            ));
        }
        let tracking_ref = refs.first().expect("one checked ref").to_string();
        if rows
            .iter()
            .any(|(_, _, source_kind)| source_kind != "remote_install")
        {
            return Err(SourcePromotionStoreError::Conflict(
                "the selected parent has a non-remote Legacy member".into(),
            ));
        }
        let mut member_paths = std::collections::BTreeSet::new();
        let mut member_ids = std::collections::BTreeSet::new();
        for (member, _, _) in &rows {
            if !member_paths.insert(member.skill_path.as_str())
                || !member_ids.insert(member.skill_id.0.as_str())
                || member.current_baseline_hash.is_empty()
            {
                return Err(SourcePromotionStoreError::Conflict(
                    "the selected parent has incomplete or ambiguous Legacy member facts".into(),
                ));
            }
        }
        let forbidden_local_link_roots = connection
            .prepare("SELECT skills_path FROM agents ORDER BY id")
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let members = rows
            .into_iter()
            .map(|(mut member, _, _)| {
                member.activations = connection
                    .prepare(
                        "SELECT agent_id, expected_entry_path, expected_target_path,
                                desired_enabled, observed_state, last_enabled_at, last_checked_at
                         FROM activations
                         WHERE skill_id = ?1
                         ORDER BY agent_id",
                    )
                    .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
                    .query_map([&member.skill_id.0], |row| {
                        Ok(SourcePromotionActivationRecord {
                            agent_id: AgentId(row.get(0)?),
                            entry_path: PathBuf::from(row.get::<_, String>(1)?),
                            target_path: PathBuf::from(row.get::<_, String>(2)?),
                            desired_enabled: row.get(3)?,
                            observed_state: parse_observed_state(&row.get::<_, String>(4)?)?,
                            last_enabled_at: row.get(5)?,
                            last_checked_at: row.get(6)?,
                        })
                    })
                    .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
                Ok(member)
            })
            .collect::<Result<Vec<_>, SourcePromotionStoreError>>()?;
        Ok(LegacySourcePromotionRecord {
            remote_id: remote_id.into(),
            canonical_url,
            aliases,
            created_at,
            tracking_ref,
            members,
            forbidden_local_link_roots,
        })
    }

    fn validate_source_promotion(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<(), SourcePromotionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        validate_source_promotion_record(&connection, record)
    }

    fn commit_source_promotion(
        &self,
        record: SourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        validate_source_promotion_record(&transaction, &record)?;

        transaction
            .execute(
                "INSERT INTO git_repository_sources (
                    remote_id, provider, canonical_url, tracking_ref, current_release_id,
                    created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, NULL,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.remote_id,
                    record.provider,
                    record.canonical_url,
                    record.tracking_ref,
                ],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "INSERT INTO git_source_releases (
                    release_id, remote_id, tracking_ref, resolved_commit, discovered_at
                 ) VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.release_id,
                    record.remote_id,
                    record.tracking_ref,
                    record.resolved_commit,
                ],
            )
            .map_err(source_promotion_sql_error)?;

        for member in &record.members {
            transaction
                .execute(
                    "INSERT INTO git_source_release_members (
                        release_id, skill_path, skill_name, tree_hash, provider_hash
                     ) VALUES (?1, ?2, ?3, ?4, NULL)",
                    params![
                        record.release_id,
                        member.skill_path,
                        member.directory_name,
                        member.remote_baseline_hash,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
            match member.origin {
                SourcePromotionMemberOrigin::Legacy => {
                    let changed = transaction
                        .execute(
                            "UPDATE skills
                             SET display_name = ?1,
                                 description = ?2,
                                 source_kind = 'remote_install',
                                 library_entry_path = ?3,
                                 final_entity_path = ?3,
                                 recorded_content_hash = ?4,
                                 health = ?5,
                                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             WHERE id = ?6",
                            params![
                                member.display_name,
                                member.description,
                                member.final_entity_path.to_string_lossy(),
                                member.current_baseline_hash,
                                health_value(member.health),
                                member.skill_id.0,
                            ],
                        )
                        .map_err(source_promotion_sql_error)?;
                    if changed != 1 {
                        return Err(SourcePromotionStoreError::Conflict(format!(
                            "Legacy member '{}' disappeared during Source Promotion",
                            member.directory_name
                        )));
                    }
                }
                SourcePromotionMemberOrigin::New => {
                    transaction
                        .execute(
                            "INSERT INTO skills (
                                id, directory_name, identity_key, display_name, description,
                                source_kind, library_entry_path, final_entity_path,
                                recorded_content_hash, health, created_at, updated_at
                             ) VALUES (
                                ?1, ?2, ?3, ?4, ?5,
                                'remote_install', ?6, ?6, ?7, ?8,
                                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             )",
                            params![
                                member.skill_id.0,
                                member.directory_name,
                                member.identity_key,
                                member.display_name,
                                member.description,
                                member.final_entity_path.to_string_lossy(),
                                member.current_baseline_hash,
                                health_value(member.health),
                            ],
                        )
                        .map_err(source_promotion_sql_error)?;
                }
            }
            transaction
                .execute(
                    "INSERT INTO git_source_members (
                        skill_id, remote_id, current_skill_path, remote_baseline_hash,
                        current_baseline_hash, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('now'), unixepoch('now'))",
                    params![
                        member.skill_id.0,
                        record.remote_id,
                        member.skill_path,
                        member.remote_baseline_hash,
                        member.current_baseline_hash,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
        }

        for removed in &record.removed_members {
            match removed {
                SourcePromotionRemovedMemberRecord::Remove { skill_id } => {
                    let deleted = transaction
                        .execute("DELETE FROM skills WHERE id = ?1", [&skill_id.0])
                        .map_err(source_promotion_sql_error)?;
                    if deleted != 1 {
                        return Err(SourcePromotionStoreError::Conflict(format!(
                            "Legacy removed member '{}' disappeared during Source Promotion",
                            skill_id.0
                        )));
                    }
                }
                SourcePromotionRemovedMemberRecord::LocalLink {
                    skill_id,
                    final_entity_path,
                } => {
                    let updated = transaction
                        .execute(
                            "UPDATE skills
                             SET source_kind = 'link',
                                 library_entry_path = NULL,
                                 final_entity_path = ?1,
                                 recorded_content_hash = NULL,
                                 health = 'healthy',
                                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             WHERE id = ?2",
                            params![final_entity_path.to_string_lossy(), skill_id.0],
                        )
                        .map_err(source_promotion_sql_error)?;
                    if updated != 1 {
                        return Err(SourcePromotionStoreError::Conflict(format!(
                            "Legacy Local Link member '{}' disappeared during Source Promotion",
                            skill_id.0
                        )));
                    }
                    transaction
                        .execute(
                            "UPDATE activations SET expected_target_path = ?1 WHERE skill_id = ?2",
                            params![final_entity_path.to_string_lossy(), skill_id.0],
                        )
                        .map_err(source_promotion_sql_error)?;
                }
            }
        }
        transaction
            .execute(
                "DELETE FROM remote_bindings WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "UPDATE git_repository_sources
                 SET current_release_id = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE remote_id = ?1",
                params![record.remote_id, record.release_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(source_promotion_sql_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(source_promotion_sql_error)?;
        transaction.commit().map_err(source_promotion_sql_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            SourcePromotionStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    fn source_promotion_is_committed(
        &self,
        record: &SourcePromotionRecord,
    ) -> Result<bool, SourcePromotionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        source_promotion_matches(&connection, record)
    }

    fn undo_source_promotion(
        &self,
        record: &SourcePromotionRecord,
        legacy: &LegacySourcePromotionRecord,
    ) -> Result<u64, SourcePromotionStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourcePromotionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(source_promotion_sql_error)?;
        if !source_promotion_matches(&transaction, record)? {
            return Err(SourcePromotionStoreError::Conflict(
                "the Source Release no longer matches the frozen Source Promotion result".into(),
            ));
        }

        transaction
            .execute(
                "UPDATE git_repository_sources SET current_release_id = NULL WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "DELETE FROM git_source_releases WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "DELETE FROM git_source_members WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "DELETE FROM git_repository_sources WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;

        for member in &record.members {
            if member.origin == SourcePromotionMemberOrigin::New {
                transaction
                    .execute("DELETE FROM skills WHERE id = ?1", [&member.skill_id.0])
                    .map_err(source_promotion_sql_error)?;
            }
        }
        for member in &legacy.members {
            let updated = transaction
                .execute(
                    "UPDATE skills
                     SET directory_name = ?1, identity_key = ?2, display_name = ?3,
                         description = ?4, source_kind = 'remote_install',
                         library_entry_path = ?5, final_entity_path = ?6,
                         recorded_content_hash = ?7, health = ?8,
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?9",
                    params![
                        member.directory_name,
                        member.identity_key,
                        member.display_name,
                        member.description,
                        member.library_entry_path.to_string_lossy(),
                        member.final_entity_path.to_string_lossy(),
                        member.recorded_content_hash,
                        health_value(member.health),
                        member.skill_id.0,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
            if updated == 0 {
                transaction
                    .execute(
                        "INSERT INTO skills (
                            id, directory_name, identity_key, display_name, description,
                            source_kind, library_entry_path, final_entity_path,
                            recorded_content_hash, health, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?7, ?8, ?9,
                                   strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                                   strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                        params![
                            member.skill_id.0,
                            member.directory_name,
                            member.identity_key,
                            member.display_name,
                            member.description,
                            member.library_entry_path.to_string_lossy(),
                            member.final_entity_path.to_string_lossy(),
                            member.recorded_content_hash,
                            health_value(member.health),
                        ],
                    )
                    .map_err(source_promotion_sql_error)?;
            }
            transaction
                .execute(
                    "DELETE FROM activations WHERE skill_id = ?1",
                    [&member.skill_id.0],
                )
                .map_err(source_promotion_sql_error)?;
            for activation in &member.activations {
                transaction
                    .execute(
                        "INSERT INTO activations (
                            skill_id, agent_id, desired_enabled, expected_entry_path,
                            expected_target_path, observed_state, last_enabled_at, last_checked_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            member.skill_id.0,
                            activation.agent_id.0,
                            activation.desired_enabled,
                            activation.entry_path.to_string_lossy(),
                            activation.target_path.to_string_lossy(),
                            observed_state_value(activation.observed_state),
                            activation.last_enabled_at,
                            activation.last_checked_at,
                        ],
                    )
                    .map_err(source_promotion_sql_error)?;
            }
            transaction
                .execute(
                    "INSERT INTO remote_bindings (
                        skill_id, remote_id, requested_ref, verification_anchor_commit,
                        original_commit_known, skill_path, provider_hash,
                        remote_baseline_hash, current_baseline_hash, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        member.skill_id.0,
                        legacy.remote_id,
                        member.requested_ref,
                        member.verification_anchor_commit,
                        member.original_commit_known,
                        member.skill_path,
                        member.provider_hash,
                        member.remote_baseline_hash,
                        member.current_baseline_hash,
                        member.last_checked_at,
                        member.last_updated_at,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(source_promotion_sql_error)?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(source_promotion_sql_error)?;
        transaction.commit().map_err(source_promotion_sql_error)?;
        u64::try_from(snapshot_version).map_err(|_| {
            SourcePromotionStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }
}

impl SourceTransitionStore for SqliteCatalogStore {
    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        validate_new_source_transition(&connection, record)
    }

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        if record.members.is_empty() {
            return Err(SourceTransitionStoreError::Conflict(
                "a Source Release must contain at least one member".into(),
            ));
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;

        let source_exists: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM remote_source_parents WHERE canonical_url = ?1
                 )",
                [&record.canonical_url],
                |row| row.get(0),
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        if source_exists {
            if source_transition_matches(&transaction, &record)? {
                let snapshot_version: i64 = transaction
                    .query_row(
                        "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
                transaction
                    .commit()
                    .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
                return u64::try_from(snapshot_version).map_err(|_| {
                    SourceTransitionStoreError::Unavailable(
                        "negative SQLite snapshot version".into(),
                    )
                });
            }
            return Err(SourceTransitionStoreError::Conflict(format!(
                "the canonical repository '{}' already has a source parent",
                record.canonical_url
            )));
        }
        for member in &record.members {
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT directory_name FROM skills
                     WHERE id = ?1 OR directory_name = ?2 OR identity_key = ?3
                     LIMIT 1",
                    params![
                        member.skill_id.0,
                        member.directory_name,
                        member.identity_key
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            if let Some(directory_name) = existing {
                return Err(SourceTransitionStoreError::Conflict(format!(
                    "the Library already contains Managed Skill '{directory_name}'"
                )));
            }
        }

        transaction
            .execute(
                "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                 VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![record.remote_id, record.canonical_url],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO git_repository_sources (
                    remote_id, provider, canonical_url, tracking_ref, current_release_id,
                    created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, NULL,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.remote_id,
                    record.provider,
                    record.canonical_url,
                    record.tracking_ref,
                ],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO git_source_releases (
                    release_id, remote_id, tracking_ref, resolved_commit, discovered_at
                 ) VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.release_id,
                    record.remote_id,
                    record.tracking_ref,
                    record.resolved_commit,
                ],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        for member in &record.members {
            transaction
                .execute(
                    "INSERT INTO git_source_release_members (
                        release_id, skill_path, skill_name, tree_hash, provider_hash
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        record.release_id,
                        member.skill_path,
                        member.directory_name,
                        member.tree_hash,
                        member.provider_hash,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5,
                        'remote_install', ?6, ?7, ?8, 'healthy',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     )",
                    params![
                        member.skill_id.0,
                        member.directory_name,
                        member.identity_key,
                        member.display_name,
                        member.description,
                        member.library_entry_path.to_string_lossy(),
                        member.final_entity_path.to_string_lossy(),
                        member.tree_hash,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO git_source_members (
                        skill_id, remote_id, current_skill_path, remote_baseline_hash,
                        current_baseline_hash, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('now'), unixepoch('now'))",
                    params![
                        member.skill_id.0,
                        record.remote_id,
                        member.skill_path,
                        member.tree_hash,
                        member.tree_hash,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        }
        transaction
            .execute(
                "UPDATE git_repository_sources
                 SET current_release_id = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE remote_id = ?1",
                params![record.remote_id, record.release_id],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        u64::try_from(snapshot_version).map_err(|_| {
            SourceTransitionStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }

    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        source_transition_matches(&connection, record)
    }

    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        if !source_transition_matches(&transaction, record)? {
            return Err(SourceTransitionStoreError::Conflict(
                "the Source Release no longer matches the frozen Source Undo result".into(),
            ));
        }
        for member in &record.members {
            let deleted = transaction
                .execute("DELETE FROM skills WHERE id = ?1", [&member.skill_id.0])
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            if deleted != 1 {
                return Err(SourceTransitionStoreError::Conflict(format!(
                    "Managed Skill '{}' is no longer present",
                    member.directory_name
                )));
            }
        }
        let deleted = transaction
            .execute(
                "DELETE FROM remote_source_parents WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        if deleted != 1 {
            return Err(SourceTransitionStoreError::Conflict(
                "the Git Repository Source is no longer present".into(),
            ));
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        let snapshot_version: i64 = transaction
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .commit()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        u64::try_from(snapshot_version).map_err(|_| {
            SourceTransitionStoreError::Unavailable("negative SQLite snapshot version".into())
        })
    }
}

/// Exact current-release test for Promotion recovery and Source Undo. This
/// intentionally compares only new Git Repository Source facts; frozen
/// Legacy binding facts remain journal/audit evidence, never release truth.
type SourcePromotionCurrentMemberState = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn source_promotion_matches(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<bool, SourcePromotionStoreError> {
    let source: Option<(String, String, String, Option<String>)> = connection
        .query_row(
            "SELECT remote_id, provider, tracking_ref, current_release_id
             FROM git_repository_sources WHERE remote_id = ?1 AND canonical_url = ?2",
            params![record.remote_id, record.canonical_url],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    if source
        != Some((
            record.remote_id.clone(),
            record.provider.clone(),
            record.tracking_ref.clone(),
            Some(record.release_id.clone()),
        ))
    {
        return Ok(false);
    }
    let release: Option<(String, String, String)> = connection
        .query_row(
            "SELECT remote_id, tracking_ref, resolved_commit
             FROM git_source_releases WHERE release_id = ?1",
            [&record.release_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    if release
        != Some((
            record.remote_id.clone(),
            record.tracking_ref.clone(),
            record.resolved_commit.clone(),
        ))
    {
        return Ok(false);
    }
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_members WHERE remote_id = ?1",
            [&record.remote_id],
            |row| row.get(0),
        )
        .map_err(source_promotion_sql_error)?;
    if count != record.members.len() as i64 {
        return Ok(false);
    }
    for member in &record.members {
        let actual: Option<SourcePromotionCurrentMemberState> = connection
            .query_row(
                "SELECT s.directory_name, s.identity_key, s.final_entity_path,
                            s.recorded_content_hash, s.health, gm.current_skill_path,
                            gm.remote_baseline_hash, gm.current_baseline_hash
                     FROM skills s
                     JOIN git_source_members gm ON gm.skill_id = s.id
                     WHERE s.id = ?1 AND gm.remote_id = ?2",
                params![member.skill_id.0, record.remote_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(source_promotion_sql_error)?;
        let expected = (
            member.directory_name.clone(),
            member.identity_key.clone(),
            member.final_entity_path.to_string_lossy().into_owned(),
            member.current_baseline_hash.clone(),
            health_value(member.health).into(),
            member.skill_path.clone(),
            member.remote_baseline_hash.clone(),
            member.current_baseline_hash.clone(),
        );
        if actual != Some(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_source_promotion_record(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<(), SourcePromotionStoreError> {
    if record.remote_id.is_empty()
        || record.provider.is_empty()
        || record.canonical_url.is_empty()
        || record.tracking_ref.is_empty()
        || record.release_id.is_empty()
        || record.resolved_commit.is_empty()
        || record.operation_id.is_empty()
        || record.members.is_empty()
    {
        return Err(SourcePromotionStoreError::Conflict(
            "the Source Promotion record is missing whole-source facts".into(),
        ));
    }
    let parent: Option<(String, String)> = connection
        .query_row(
            "SELECT canonical_url, created_at FROM remote_source_parents WHERE remote_id = ?1",
            [&record.remote_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    let aliases = connection
        .prepare(
            "SELECT alias_url FROM remote_source_aliases
             WHERE remote_id = ?1 ORDER BY alias_url",
        )
        .map_err(source_promotion_sql_error)?
        .query_map([&record.remote_id], |row| row.get::<_, String>(0))
        .map_err(source_promotion_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(source_promotion_sql_error)?;
    if parent
        != Some((
            record.canonical_url.clone(),
            record.legacy.created_at.clone(),
        ))
        || aliases != record.legacy.aliases
    {
        return Err(SourcePromotionStoreError::Conflict(
            "the Legacy parent no longer matches the frozen source facts".into(),
        ));
    }
    let promoted: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM git_repository_sources WHERE remote_id = ?1)",
            [&record.remote_id],
            |row| row.get(0),
        )
        .map_err(source_promotion_sql_error)?;
    if promoted {
        return Err(SourcePromotionStoreError::Conflict(
            "the selected parent is no longer Legacy Per-Skill Git State".into(),
        ));
    }

    let actual_legacy_ids = connection
        .prepare("SELECT skill_id FROM remote_bindings WHERE remote_id = ?1 ORDER BY skill_id")
        .map_err(source_promotion_sql_error)?
        .query_map([&record.remote_id], |row| row.get::<_, String>(0))
        .map_err(source_promotion_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(source_promotion_sql_error)?;
    let expected_legacy_ids = record
        .legacy_member_ids
        .iter()
        .map(|id| id.0.clone())
        .collect::<BTreeSet<_>>();
    if expected_legacy_ids.len() != record.legacy_member_ids.len()
        || actual_legacy_ids.len() != expected_legacy_ids.len()
        || actual_legacy_ids.iter().collect::<BTreeSet<_>>()
            != expected_legacy_ids.iter().collect::<BTreeSet<_>>()
    {
        return Err(SourcePromotionStoreError::Conflict(
            "the Legacy member set changed after the Source Group Draft".into(),
        ));
    }
    if record.legacy.remote_id != record.remote_id
        || record.legacy.canonical_url != record.canonical_url
        || record.legacy.tracking_ref != record.tracking_ref
        || record.legacy.members.len() != expected_legacy_ids.len()
    {
        return Err(SourcePromotionStoreError::Conflict(
            "the frozen Legacy source facts do not match the promotion target".into(),
        ));
    }
    for legacy in &record.legacy.members {
        // Keep this as a named value rather than a long tuple: standard
        // library tuple comparisons stop before this many facts, and losing
        // any one of these fields would turn a Legacy audit snapshot into a
        // stale write precondition.
        let actual: Option<LegacyPromotionBindingFacts> = connection
            .query_row(
                "SELECT b.requested_ref, b.verification_anchor_commit,
                        b.original_commit_known, b.skill_path, b.provider_hash,
                        b.remote_baseline_hash, b.current_baseline_hash,
                        b.last_checked_at, b.last_updated_at,
                        s.source_kind, s.directory_name, s.identity_key,
                        s.display_name, s.description, s.library_entry_path,
                        s.final_entity_path, s.recorded_content_hash, s.health
                 FROM remote_bindings b
                 JOIN skills s ON s.id = b.skill_id
                 WHERE b.remote_id = ?1 AND b.skill_id = ?2",
                params![record.remote_id, legacy.skill_id.0],
                |row| {
                    Ok(LegacyPromotionBindingFacts {
                        requested_ref: row.get(0)?,
                        verification_anchor_commit: row.get(1)?,
                        original_commit_known: row.get(2)?,
                        skill_path: row.get(3)?,
                        provider_hash: row.get(4)?,
                        remote_baseline_hash: row.get(5)?,
                        current_baseline_hash: row.get(6)?,
                        last_checked_at: row.get(7)?,
                        last_updated_at: row.get(8)?,
                        source_kind: row.get(9)?,
                        directory_name: row.get(10)?,
                        identity_key: row.get(11)?,
                        display_name: row.get(12)?,
                        description: row.get(13)?,
                        library_entry_path: row.get(14)?,
                        final_entity_path: row.get(15)?,
                        recorded_content_hash: row.get(16)?,
                        health: row.get(17)?,
                    })
                },
            )
            .optional()
            .map_err(source_promotion_sql_error)?;
        let expected = LegacyPromotionBindingFacts {
            requested_ref: legacy.requested_ref.clone(),
            verification_anchor_commit: legacy.verification_anchor_commit.clone(),
            original_commit_known: legacy.original_commit_known,
            skill_path: legacy.skill_path.clone(),
            provider_hash: legacy.provider_hash.clone(),
            remote_baseline_hash: legacy.remote_baseline_hash.clone(),
            current_baseline_hash: legacy.current_baseline_hash.clone(),
            last_checked_at: legacy.last_checked_at,
            last_updated_at: legacy.last_updated_at,
            source_kind: "remote_install".into(),
            directory_name: legacy.directory_name.clone(),
            identity_key: legacy.identity_key.clone(),
            display_name: legacy.display_name.clone(),
            description: legacy.description.clone(),
            library_entry_path: legacy.library_entry_path.to_string_lossy().into_owned(),
            final_entity_path: legacy.final_entity_path.to_string_lossy().into_owned(),
            recorded_content_hash: legacy.recorded_content_hash.clone(),
            health: health_value(legacy.health).into(),
        };
        if actual != Some(expected) {
            return Err(SourcePromotionStoreError::Conflict(
                "a frozen Legacy binding changed after the Source Group Draft".into(),
            ));
        }
        let activations = connection
            .prepare(
                "SELECT agent_id, expected_entry_path, expected_target_path,
                        desired_enabled, observed_state, last_enabled_at, last_checked_at
                 FROM activations WHERE skill_id = ?1 ORDER BY agent_id",
            )
            .map_err(source_promotion_sql_error)?
            .query_map([&legacy.skill_id.0], |row| {
                Ok(SourcePromotionActivationRecord {
                    agent_id: AgentId(row.get(0)?),
                    entry_path: PathBuf::from(row.get::<_, String>(1)?),
                    target_path: PathBuf::from(row.get::<_, String>(2)?),
                    desired_enabled: row.get(3)?,
                    observed_state: parse_observed_state(&row.get::<_, String>(4)?)?,
                    last_enabled_at: row.get(5)?,
                    last_checked_at: row.get(6)?,
                })
            })
            .map_err(source_promotion_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(source_promotion_sql_error)?;
        if activations != legacy.activations {
            return Err(SourcePromotionStoreError::Conflict(
                "a frozen Legacy Activation changed after the Source Group Draft".into(),
            ));
        }
    }

    let mut consumed_legacy = BTreeSet::new();
    let mut target_paths = BTreeSet::new();
    let mut directory_names = BTreeSet::new();
    let mut identity_keys = BTreeSet::new();
    let mut member_ids = BTreeSet::new();
    for member in &record.members {
        if member.skill_id.0.is_empty()
            || member.directory_name.is_empty()
            || member.identity_key.is_empty()
            || member.remote_baseline_hash.is_empty()
            || member.current_baseline_hash.is_empty()
            || !target_paths.insert(member.skill_path.as_str())
            || !directory_names.insert(member.directory_name.as_str())
            || !identity_keys.insert(member.identity_key.as_str())
            || !member_ids.insert(member.skill_id.0.as_str())
        {
            return Err(SourcePromotionStoreError::Conflict(
                "the target Source Release contains incomplete or duplicate members".into(),
            ));
        }
        match member.origin {
            SourcePromotionMemberOrigin::Legacy => {
                if !expected_legacy_ids.contains(&member.skill_id.0)
                    || !consumed_legacy.insert(member.skill_id.0.as_str())
                {
                    return Err(SourcePromotionStoreError::Conflict(
                        "a target Source Member does not map to one selected Legacy member".into(),
                    ));
                }
                let existing: Option<(String, String, String)> = connection
                    .query_row(
                        "SELECT directory_name, identity_key, final_entity_path
                         FROM skills WHERE id = ?1",
                        [&member.skill_id.0],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(source_promotion_sql_error)?;
                let Some((directory_name, identity_key, final_entity_path)) = existing else {
                    return Err(SourcePromotionStoreError::Conflict(
                        "a selected Legacy member disappeared".into(),
                    ));
                };
                if directory_name != member.directory_name
                    || identity_key != member.identity_key
                    || final_entity_path != member.final_entity_path.to_string_lossy()
                {
                    return Err(SourcePromotionStoreError::Conflict(
                        "a selected Legacy member changed identity or location".into(),
                    ));
                }
            }
            SourcePromotionMemberOrigin::New => {
                let collision: bool = connection
                    .query_row(
                        "SELECT EXISTS(
                            SELECT 1 FROM skills
                            WHERE id = ?1 OR directory_name = ?2 OR identity_key = ?3
                         )",
                        params![
                            member.skill_id.0,
                            member.directory_name,
                            member.identity_key
                        ],
                        |row| row.get(0),
                    )
                    .map_err(source_promotion_sql_error)?;
                if collision {
                    return Err(SourcePromotionStoreError::Conflict(
                        "a new target Source Member conflicts with an existing Managed Skill"
                            .into(),
                    ));
                }
            }
        }
    }
    for removed in &record.removed_members {
        let (skill_id, valid_link_target) = match removed {
            SourcePromotionRemovedMemberRecord::Remove { skill_id } => (skill_id, true),
            SourcePromotionRemovedMemberRecord::LocalLink {
                skill_id,
                final_entity_path,
            } => (skill_id, !final_entity_path.as_os_str().is_empty()),
        };
        if !valid_link_target
            || !expected_legacy_ids.contains(&skill_id.0)
            || !consumed_legacy.insert(skill_id.0.as_str())
        {
            return Err(SourcePromotionStoreError::Conflict(
                "a removed Legacy member is missing, duplicated, or unresolved".into(),
            ));
        }
    }
    if consumed_legacy.len() != expected_legacy_ids.len() {
        return Err(SourcePromotionStoreError::Conflict(
            "every selected Legacy member must become a Source Member or an explicit removal"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct LegacyPromotionBindingFacts {
    requested_ref: String,
    verification_anchor_commit: String,
    original_commit_known: bool,
    skill_path: String,
    provider_hash: Option<String>,
    remote_baseline_hash: String,
    current_baseline_hash: String,
    last_checked_at: Option<i64>,
    last_updated_at: Option<i64>,
    source_kind: String,
    directory_name: String,
    identity_key: String,
    display_name: String,
    description: String,
    library_entry_path: String,
    final_entity_path: String,
    recorded_content_hash: String,
    health: String,
}

fn source_promotion_sql_error(error: rusqlite::Error) -> SourcePromotionStoreError {
    SourcePromotionStoreError::Unavailable(error.to_string())
}

fn validate_new_source_transition(
    connection: &Connection,
    record: &SourceTransitionRecord,
) -> Result<(), SourceTransitionStoreError> {
    if record.members.is_empty() {
        return Err(SourceTransitionStoreError::Conflict(
            "a Source Release must contain at least one member".into(),
        ));
    }
    let source_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_source_parents WHERE canonical_url = ?1)",
            [&record.canonical_url],
            |row| row.get(0),
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    if source_exists {
        return Err(SourceTransitionStoreError::Conflict(format!(
            "the canonical repository '{}' already has a source parent",
            record.canonical_url
        )));
    }
    for member in &record.members {
        let existing: Option<String> = connection
            .query_row(
                "SELECT directory_name FROM skills
                 WHERE id = ?1 OR directory_name = ?2 OR identity_key = ?3
                 LIMIT 1",
                params![
                    member.skill_id.0,
                    member.directory_name,
                    member.identity_key
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        if let Some(directory_name) = existing {
            return Err(SourceTransitionStoreError::Conflict(format!(
                "the Library already contains Managed Skill '{directory_name}'"
            )));
        }
    }
    Ok(())
}

/// Exact, source-level state check shared by idempotent post-CAS recovery and
/// Source Undo. A legacy binding, a changed current member, or a later source
/// release all make this false; callers then leave the whole source untouched.
type SourceTransitionMemberState = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

fn source_transition_matches(
    connection: &Connection,
    record: &SourceTransitionRecord,
) -> Result<bool, SourceTransitionStoreError> {
    if record.members.is_empty() {
        return Ok(false);
    }
    let source: Option<(String, String, String, Option<String>)> = connection
        .query_row(
            "SELECT remote_id, provider, tracking_ref, current_release_id
             FROM git_repository_sources WHERE canonical_url = ?1",
            [&record.canonical_url],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let Some((remote_id, provider, tracking_ref, current_release_id)) = source else {
        return Ok(false);
    };
    if remote_id != record.remote_id
        || provider != record.provider
        || tracking_ref != record.tracking_ref
        || current_release_id.as_deref() != Some(record.release_id.as_str())
    {
        return Ok(false);
    }
    let release: Option<(String, String)> = connection
        .query_row(
            "SELECT remote_id, resolved_commit FROM git_source_releases WHERE release_id = ?1",
            [&record.release_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    if release != Some((record.remote_id.clone(), record.resolved_commit.clone())) {
        return Ok(false);
    }
    let release_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_release_members WHERE release_id = ?1",
            [&record.release_id],
            |row| row.get(0),
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let current_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM git_source_members WHERE remote_id = ?1",
            [&record.remote_id],
            |row| row.get(0),
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    if release_count != record.members.len() as i64 || current_count != record.members.len() as i64
    {
        return Ok(false);
    }
    for member in &record.members {
        let actual: Option<SourceTransitionMemberState> = connection
            .query_row(
                "SELECT s.directory_name, s.identity_key, s.display_name, s.description,
                        s.library_entry_path, s.final_entity_path, s.recorded_content_hash,
                        gm.current_skill_path, gm.remote_baseline_hash, gm.current_baseline_hash,
                        rm.tree_hash,
                        rm.provider_hash
                 FROM skills s
                 JOIN git_source_members gm ON gm.skill_id = s.id
                 JOIN git_source_release_members rm
                   ON rm.release_id = ?2 AND rm.skill_path = gm.current_skill_path
                 WHERE s.id = ?1 AND gm.remote_id = ?3",
                params![member.skill_id.0, record.release_id, record.remote_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        let expected = (
            member.directory_name.clone(),
            member.identity_key.clone(),
            member.display_name.clone(),
            member.description.clone(),
            member.library_entry_path.to_string_lossy().into_owned(),
            member.final_entity_path.to_string_lossy().into_owned(),
            member.tree_hash.clone(),
            member.skill_path.clone(),
            member.tree_hash.clone(),
            member.tree_hash.clone(),
            member.tree_hash.clone(),
            member.provider_hash.clone(),
        );
        if actual != Some(expected) {
            return Ok(false);
        }
    }
    Ok(true)
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
            // Parent per canonical URL: reuse the existing parent when one
            // already exists (ADR-0013 §4.2), otherwise keep the fresh id.
            transaction
                .execute(
                    "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                     VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ON CONFLICT(canonical_url) DO NOTHING",
                    params![record.remote_id, record.source_url],
                )
                .map_err(sqlite_import_error)?;
            let remote_id: String = transaction
                .query_row(
                    "SELECT remote_id FROM remote_source_parents WHERE canonical_url = ?1",
                    [&record.source_url],
                    |row| row.get(0),
                )
                .map_err(sqlite_import_error)?;
            transaction
                .execute(
                    "INSERT INTO remote_bindings (
                        skill_id, remote_id, requested_ref, verification_anchor_commit,
                        original_commit_known, skill_path, provider_hash,
                        remote_baseline_hash, current_baseline_hash,
                        last_checked_at, last_updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                        unixepoch('now'),
                        unixepoch('now')
                     )",
                    params![
                        record.skill_id.0,
                        remote_id,
                        record.requested_ref,
                        record.verification_anchor_commit,
                        record.original_commit_known as i64,
                        record.skill_path,
                        record.provider_hash,
                        record.remote_baseline_hash,
                        record.current_baseline_hash,
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
                        skills.health, remote_source_parents.canonical_url,
                        remote_bindings.remote_id, remote_bindings.requested_ref,
                        remote_bindings.verification_anchor_commit,
                        remote_bindings.original_commit_known, remote_bindings.skill_path,
                        remote_bindings.provider_hash, remote_bindings.remote_baseline_hash,
                        remote_bindings.current_baseline_hash, remote_bindings.last_checked_at,
                        remote_bindings.last_updated_at
                   FROM skills
                   JOIN remote_bindings ON remote_bindings.skill_id = skills.id
                   JOIN remote_source_parents
                     ON remote_source_parents.remote_id = remote_bindings.remote_id
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
                    remote_id: row.get(9)?,
                    requested_ref: row.get(10)?,
                    verification_anchor_commit: row.get(11)?,
                    original_commit_known: row.get::<_, bool>(12)?,
                    skill_path: row.get(13)?,
                    provider_hash: row.get(14)?,
                    remote_baseline_hash: row.get(15)?,
                    current_baseline_hash: row.get(16)?,
                    last_checked_at: row.get::<_, Option<i64>>(17)?,
                    last_updated_at: row.get::<_, Option<i64>>(18)?,
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
                        skills.health, remote_source_parents.canonical_url,
                        remote_bindings.remote_id, remote_bindings.requested_ref,
                        remote_bindings.verification_anchor_commit,
                        remote_bindings.original_commit_known, remote_bindings.skill_path,
                        remote_bindings.provider_hash, remote_bindings.remote_baseline_hash,
                        remote_bindings.current_baseline_hash, remote_bindings.last_checked_at,
                        remote_bindings.last_updated_at
                   FROM skills
                   JOIN remote_bindings ON remote_bindings.skill_id = skills.id
                   JOIN remote_source_parents
                     ON remote_source_parents.remote_id = remote_bindings.remote_id
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
                        remote_id: row.get(9)?,
                        requested_ref: row.get(10)?,
                        verification_anchor_commit: row.get(11)?,
                        original_commit_known: row.get::<_, bool>(12)?,
                        skill_path: row.get(13)?,
                        provider_hash: row.get(14)?,
                        remote_baseline_hash: row.get(15)?,
                        current_baseline_hash: row.get(16)?,
                        last_checked_at: row.get::<_, Option<i64>>(17)?,
                        last_updated_at: row.get::<_, Option<i64>>(18)?,
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
                "UPDATE remote_bindings
                    SET verification_anchor_commit = ?2, skill_path = ?3,
                        original_commit_known = ?4, provider_hash = ?5,
                        remote_baseline_hash = ?6, current_baseline_hash = ?7,
                        last_updated_at = unixepoch('now'),
                        last_checked_at = unixepoch('now')
                  WHERE skill_id = ?1",
                params![
                    record.skill_id.0,
                    record.verification_anchor_commit,
                    record.skill_path,
                    record.original_commit_known as i64,
                    record.provider_hash,
                    record.remote_baseline_hash,
                    record.current_baseline_hash,
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

    fn record_remote_check(&self, skill_id: &SkillId) -> Result<(), ImportStoreError> {
        self.connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                "UPDATE remote_bindings SET last_checked_at = unixepoch('now') WHERE skill_id = ?1",
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
                "UPDATE remote_bindings SET requested_ref = ?2 WHERE skill_id = ?1",
                params![skill_id.0, requested_ref],
            )
            .map_err(sqlite_import_error)?;
        Ok(())
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, ImportStoreError> {
        self.remote_parent_by_url(canonical_url)
            .map_err(ImportStoreError::Unavailable)
    }

    fn load_remote_parents(&self) -> Result<Vec<RemoteParentRecord>, ImportStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare(
                "SELECT remote_id, canonical_url, created_at
                   FROM remote_source_parents ORDER BY canonical_url",
            )
            .map_err(sqlite_import_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sqlite_import_error)?;
        let mut parents = Vec::new();
        for row in rows {
            let (remote_id, canonical_url, created_at) = row.map_err(sqlite_import_error)?;
            let aliases = connection
                .prepare(
                    "SELECT alias_url FROM remote_source_aliases WHERE remote_id = ?1 ORDER BY alias_url",
                )
                .map_err(sqlite_import_error)?
                .query_map([&remote_id], |row| row.get(0))
                .map_err(sqlite_import_error)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(sqlite_import_error)?;
            parents.push(RemoteParentRecord {
                remote_id,
                canonical_url,
                created_at,
                aliases,
            });
        }
        Ok(parents)
    }

    fn insert_remote_alias(
        &self,
        remote_id: &str,
        alias_url: &str,
    ) -> Result<(), ImportStoreError> {
        let changed = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                // Fail closed on two-parent convergence: an alias must not
                // collide with any parent's canonical URL (ADR-0013 §4.2:
                // existing parents only merge via an explicit survivor).
                "INSERT INTO remote_source_aliases (remote_id, alias_url, confirmed_at)
                 SELECT ?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE NOT EXISTS (
                            SELECT 1 FROM remote_source_parents WHERE canonical_url = ?2
                        )",
                params![remote_id, alias_url],
            )
            .map_err(sqlite_import_error)?;
        if changed != 1 {
            return Err(ImportStoreError::Unavailable(format!(
                "the alias '{alias_url}' collides with an existing parent; \
                 two parents never auto-merge — choose a survivor explicitly"
            )));
        }
        Ok(())
    }

    fn delete_remote_parent_if_last_child(
        &self,
        remote_id: &str,
    ) -> Result<bool, ImportStoreError> {
        self.remove_parent_if_last_child(remote_id)
            .map_err(ImportStoreError::Unavailable)
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

    fn insert_remote_adopted(
        &self,
        record: RemoteAdoptedSkillRecord,
    ) -> Result<u64, AdoptStoreError> {
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
        if let Some(_directory_name) = existing {
            // Crash roll-forward: the skill row already committed; report
            // the current snapshot version without touching anything.
            let snapshot_version: i64 = transaction
                .query_row(
                    "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(sqlite_adopt_error)?;
            transaction.commit().map_err(sqlite_adopt_error)?;
            return u64::try_from(snapshot_version).map_err(|_| {
                AdoptStoreError::Unavailable("negative SQLite snapshot version".into())
            });
        }
        transaction
            .execute(
                "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
                 VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 ON CONFLICT(canonical_url) DO NOTHING",
                params![record.remote_id, record.canonical_url],
            )
            .map_err(sqlite_adopt_error)?;
        let remote_id: String = transaction
            .query_row(
                "SELECT remote_id FROM remote_source_parents WHERE canonical_url = ?1",
                [&record.canonical_url],
                |row| row.get(0),
            )
            .map_err(sqlite_adopt_error)?;
        transaction
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?6, ?7, ?8,
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
                    record.recorded_content_hash,
                    health_value(record.health),
                ],
            )
            .map_err(sqlite_adopt_error)?;
        transaction
            .execute(
                "INSERT INTO remote_bindings (
                    skill_id, remote_id, requested_ref, verification_anchor_commit,
                    original_commit_known, skill_path, provider_hash,
                    remote_baseline_hash, current_baseline_hash,
                    last_checked_at, last_updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                    unixepoch('now'), unixepoch('now')
                 )",
                params![
                    record.skill_id.0,
                    remote_id,
                    record.requested_ref,
                    record.verification_anchor_commit,
                    record.original_commit_known as i64,
                    record.skill_path,
                    record.provider_hash,
                    record.remote_baseline_hash,
                    record.current_baseline_hash,
                ],
            )
            .map_err(sqlite_adopt_error)?;
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

    fn delete_remote_parent_if_last_child(&self, remote_id: &str) -> Result<bool, AdoptStoreError> {
        self.remove_parent_if_last_child(remote_id)
            .map_err(AdoptStoreError::Unavailable)
    }

    fn find_remote_parent_by_url(
        &self,
        canonical_url: &str,
    ) -> Result<Option<RemoteParentRecord>, AdoptStoreError> {
        self.remote_parent_by_url(canonical_url)
            .map_err(AdoptStoreError::Unavailable)
    }

    fn binding_remote_id(&self, skill_id: &SkillId) -> Result<Option<String>, AdoptStoreError> {
        self.connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT remote_id FROM remote_bindings WHERE skill_id = ?1",
                [skill_id.0.as_str()],
                |row| row.get(0),
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

    fn last_app_update_check_at(&self) -> Result<Option<i64>, PreferencesStoreError> {
        let connection = self
            .connection()
            .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?;
        connection
            .query_row(
                "SELECT last_app_update_check_at FROM preferences WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))?
            .map(|value| {
                value.parse::<i64>().map_err(|error| {
                    PreferencesStoreError::Unavailable(format!(
                        "invalid App Update check timestamp: {error}"
                    ))
                })
            })
            .transpose()
    }

    fn record_app_update_check_at(&self, checked_at: i64) -> Result<(), PreferencesStoreError> {
        self.connection
            .lock()
            .map_err(|_| PreferencesStoreError::Unavailable("SQLite lock poisoned".into()))?
            .execute(
                "UPDATE preferences SET last_app_update_check_at = ?1 WHERE singleton = 1",
                [checked_at.to_string()],
            )
            .map(|_| ())
            .map_err(|error| PreferencesStoreError::Unavailable(error.to_string()))
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
    if existing_schema_version == 4 {
        // Schema v5: Home identity (spec §3.4). Exclusive to the Home
        // Binding/Legacy transition; ordinary open() never reaches this.
        transaction.execute_batch(
            "ALTER TABLE catalog_meta ADD COLUMN home_id TEXT;
             ALTER TABLE catalog_meta ADD COLUMN volume_fsid TEXT;
             ALTER TABLE catalog_meta ADD COLUMN volume_uuid TEXT;
             ALTER TABLE catalog_meta ADD COLUMN home_bound_at TEXT;
             UPDATE catalog_meta SET schema_version = 5 WHERE singleton = 1;",
        )?;
    }
    // v4 inputs run the v5 step first in this same transaction; v5 inputs
    // already carry the identity columns. Pre-v4 chains are the Home
    // Binding flow's single-step legacy path and are untouched here.
    if existing_schema_version == 4 || existing_schema_version == 5 {
        // Schema v6: Remote Source Parent (spec §3.4, ADR-0013 §4).
        // Legacy Skill Man-owned Remote Installs keep their resolved
        // commit, ref, path and baselines: each `remote_sources` row
        // becomes one parent (per normalized URL) plus one per-Skill
        // Binding. The old table is dropped only after integrity and
        // foreign-key checks pass inside this transaction. External
        // `.skill-lock.json` files are never read or modified here.
        transaction.execute_batch(
            "CREATE TABLE remote_source_parents (
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
                original_commit_known INTEGER NOT NULL DEFAULT 0 CHECK (original_commit_known IN (0, 1)),
                skill_path TEXT NOT NULL,
                provider_hash TEXT,
                remote_baseline_hash TEXT NOT NULL,
                current_baseline_hash TEXT NOT NULL,
                last_checked_at INTEGER,
                last_updated_at INTEGER
             );",
        )?;
        migrate_legacy_remote_rows(&transaction)?;
        let violations: i64 =
            transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if violations != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "schema v6 migration left {violations} foreign-key violations"
                )),
            ));
        }
        let integrity: String =
            transaction.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
                Some("schema v6 migration failed the integrity check".into()),
            ));
        }
        transaction.execute_batch(
            "DROP TABLE remote_sources;
             UPDATE catalog_meta SET schema_version = 6 WHERE singleton = 1;",
        )?;
    }
    if matches!(existing_schema_version, 4..=6) {
        // Schema v7: Git Repository Source facts (ADR-0014). Existing v6
        // parents and per-Skill bindings deliberately remain Legacy; this
        // migration creates no repository source, release or member rows.
        transaction.execute_batch(
            "CREATE TABLE git_repository_sources (
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
             UPDATE catalog_meta SET schema_version = 7 WHERE singleton = 1;",
        )?;
    }
    transaction.commit()
}

/// Copy legacy `remote_sources` rows into parents and bindings (spec
/// §3.4): the normalized URL keys one parent per repository, the resolved
/// commit becomes the known install commit and Verification Anchor, and
/// the recorded content hash becomes both the remote and current baseline.
/// A row whose Skill has no recorded content hash fails the whole
/// migration: a baseline can never be guessed.
fn migrate_legacy_remote_rows(transaction: &rusqlite::Transaction) -> rusqlite::Result<()> {
    let mut statement = transaction.prepare(
        "SELECT skills.id, skills.recorded_content_hash, remote_sources.source_url,
                remote_sources.requested_ref, remote_sources.resolved_commit,
                remote_sources.skill_path, remote_sources.last_checked_at,
                remote_sources.last_updated_at
           FROM remote_sources
           JOIN skills ON skills.id = remote_sources.skill_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<i64>>(7)?,
        ))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    for (
        skill_id,
        recorded_hash,
        source_url,
        requested_ref,
        resolved_commit,
        skill_path,
        last_checked_at,
        last_updated_at,
    ) in rows
    {
        let canonical_url = crate::adapters::remote_provider::normalize_catalog_url(&source_url)
            .map_err(|error| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                    Some(format!(
                        "legacy remote Install '{skill_id}' has a non-normalizable URL '{source_url}': {error}"
                    )),
                )
            })?;
        let Some(recorded_hash) = recorded_hash else {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                Some(format!(
                    "legacy remote Install '{skill_id}' has no recorded content hash; \
                     its baseline cannot be migrated"
                )),
            ));
        };
        let remote_id = new_remote_id(transaction)?;
        transaction.execute(
            "INSERT INTO remote_source_parents (remote_id, canonical_url, created_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(canonical_url) DO NOTHING",
            rusqlite::params![remote_id, canonical_url],
        )?;
        let parent_id: String = transaction.query_row(
            "SELECT remote_id FROM remote_source_parents WHERE canonical_url = ?1",
            [&canonical_url],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO remote_bindings (
                skill_id, remote_id, requested_ref, verification_anchor_commit,
                original_commit_known, skill_path, provider_hash,
                remote_baseline_hash, current_baseline_hash,
                last_checked_at, last_updated_at
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, ?6, ?6, ?7, ?8)",
            rusqlite::params![
                skill_id,
                parent_id,
                requested_ref,
                resolved_commit,
                skill_path,
                recorded_hash,
                last_checked_at,
                last_updated_at
            ],
        )?;
    }
    Ok(())
}

/// A fresh remote parent id: a UUID v4 built from SQLite's random bytes,
/// shaped exactly like the Home identity ids.
fn new_remote_id(transaction: &rusqlite::Transaction) -> rusqlite::Result<String> {
    let mut bytes = [0u8; 16];
    transaction.query_row("SELECT randomblob(16)", [], |row| {
        let blob = row.get::<_, Vec<u8>>(0)?;
        let len = blob.len().min(16);
        bytes[..len].copy_from_slice(&blob[..len]);
        Ok(())
    })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Read the Home identity columns; `None` when missing, empty or invalid.
fn read_catalog_identity(connection: &Connection) -> Option<CatalogHomeIdentity> {
    let (home_id, volume_fsid, volume_uuid, home_bound_at): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT home_id, volume_fsid, volume_uuid, home_bound_at
             FROM catalog_meta WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let home_id = HomeId::parse(&home_id?)?;
    let volume_fsid = volume_fsid?;
    let volume_uuid = volume_uuid?;
    let home_bound_at = home_bound_at?;
    if volume_fsid.is_empty() || volume_uuid.is_empty() || home_bound_at.is_empty() {
        return None;
    }
    Some(CatalogHomeIdentity {
        home_id,
        volume_fsid,
        volume_uuid,
        home_bound_at,
    })
}

/// Whole-file SQLite backup taken by the Home Binding/Legacy migration flow
/// before any pre-identity schema is migrated; covered by the unit tests in
/// this module.
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

fn parse_agent_kind(value: &str) -> rusqlite::Result<AgentKind> {
    match value {
        "claude_preset" => Ok(AgentKind::ClaudePreset),
        "codex_preset" => Ok(AgentKind::CodexPreset),
        "custom" => Ok(AgentKind::Custom),
        value => Err(invalid_enum_value(3, value)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::home::BoundHome;
    use crate::seams::catalog_probe::CatalogProbe;

    fn bound_home() -> BoundHome {
        BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        )
    }

    #[test]
    fn fresh_open_creates_v5_without_identity() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let store = SqliteCatalogStore::open(&path).expect("fresh open");
        let status = store.startup_status();
        assert_eq!(status.access, StartupAccess::ReadWrite);
        assert_eq!(status.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(status.diagnostic.is_none());
        let identity = read_catalog_identity(&store.connection().expect("catalog connection"));
        assert!(identity.is_none(), "fresh Catalog has no Home identity");
        let connection = store.connection().expect("catalog connection");
        let repository_source_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name IN (
                    'git_repository_sources',
                    'git_source_releases',
                    'git_source_release_members',
                    'git_source_members'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("repository source tables");
        assert_eq!(repository_source_tables, 4);
    }

    #[test]
    fn commits_a_complete_git_source_release_and_current_members_together() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let store = SqliteCatalogStore::open(&path).expect("fresh open");
        let version = SourceTransitionStore::commit_source_transition(
            &store,
            SourceTransitionRecord {
                remote_id: "remote-source-1".into(),
                provider: "github".into(),
                canonical_url: "https://github.com/acme/source".into(),
                tracking_ref: "main".into(),
                release_id: "release-1".into(),
                resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
                members: vec![
                    crate::seams::source_transition_store::SourceTransitionMemberRecord {
                        skill_id: SkillId("source-skill-a".into()),
                        directory_name: "alpha".into(),
                        identity_key: "alpha".into(),
                        display_name: "Alpha".into(),
                        description: "First source member".into(),
                        library_entry_path: PathBuf::from("/Library/skills/alpha"),
                        final_entity_path: PathBuf::from("/Library/skills/alpha"),
                        skill_path: "skills/alpha".into(),
                        tree_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                        provider_hash: None,
                    },
                    crate::seams::source_transition_store::SourceTransitionMemberRecord {
                        skill_id: SkillId("source-skill-b".into()),
                        directory_name: "beta".into(),
                        identity_key: "beta".into(),
                        display_name: "Beta".into(),
                        description: "Second source member".into(),
                        library_entry_path: PathBuf::from("/Library/skills/beta"),
                        final_entity_path: PathBuf::from("/Library/skills/beta"),
                        skill_path: "skills/beta".into(),
                        tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                        provider_hash: Some("provider-hash".into()),
                    },
                ],
            },
        )
        .expect("commit source transition");
        assert_eq!(version, 1);

        let connection = store.connection().expect("catalog connection");
        let current_release: String = connection
            .query_row(
                "SELECT current_release_id FROM git_repository_sources
                 WHERE remote_id = 'remote-source-1'",
                [],
                |row| row.get(0),
            )
            .expect("current release");
        assert_eq!(current_release, "release-1");
        let release_member_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM git_source_release_members WHERE release_id = 'release-1'",
                [],
                |row| row.get(0),
            )
            .expect("release member count");
        let current_member_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM git_source_members WHERE remote_id = 'remote-source-1'",
                [],
                |row| row.get(0),
            )
            .expect("current member count");
        assert_eq!(release_member_count, 2);
        assert_eq!(current_member_count, 2);
        let independent_member_versions: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM remote_bindings WHERE skill_id IN ('source-skill-a', 'source-skill-b')",
                [],
                |row| row.get(0),
            )
            .expect("legacy binding count");
        assert_eq!(independent_member_versions, 0);
    }

    #[test]
    fn v4_catalog_opens_read_only_with_migration_required() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            let connection = Connection::open(&path).expect("create v4 catalog");
            connection
                .execute_batch(
                    "CREATE TABLE catalog_meta (
                        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                        schema_version INTEGER NOT NULL,
                        snapshot_version INTEGER NOT NULL DEFAULT 0,
                        first_run_completed_at TEXT,
                        last_startup_check_at TEXT
                     );
                     INSERT INTO catalog_meta (singleton, schema_version) VALUES (1, 4);",
                )
                .expect("seed v4 header");
        }
        let store = SqliteCatalogStore::open(&path).expect("open v4 read-only");
        let status = store.startup_status();
        assert_eq!(status.access, StartupAccess::ReadOnly);
        assert_eq!(status.schema_version, 4);
        assert_eq!(
            status.diagnostic.expect("diagnostic").code,
            StartupDiagnosticCode::MigrationRequired
        );
        // Ordinary open must not migrate or back up the v4 file.
        assert!(!path.with_extension("pre-migration-v4.bak").exists());
    }

    #[test]
    fn migrate_v4_to_current_preserves_rows_and_adds_identity_columns() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            // The exact pre-identity v4 shape: the v5 schema minus the
            // identity columns.
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("open v4 catalog");
            connection
                .execute_batch(
                    "ALTER TABLE catalog_meta DROP COLUMN home_id;
                     ALTER TABLE catalog_meta DROP COLUMN volume_fsid;
                     ALTER TABLE catalog_meta DROP COLUMN volume_uuid;
                     ALTER TABLE catalog_meta DROP COLUMN home_bound_at;
                     UPDATE catalog_meta SET schema_version = 4,
                        first_run_completed_at = '2026-08-01T00:00:00Z'
                      WHERE singleton = 1;",
                )
                .expect("rewind to the v4 shape");
        }
        let mut connection = Connection::open(&path).expect("reopen");
        migrate_to_current(&mut connection, 4).expect("v4 → current migration");
        let schema: u32 = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema version");
        assert_eq!(schema, CURRENT_SCHEMA_VERSION);
        let first_run: Option<String> = connection
            .query_row(
                "SELECT first_run_completed_at FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("first run flag preserved");
        assert_eq!(first_run.as_deref(), Some("2026-08-01T00:00:00Z"));
        let home_id: Option<String> = connection
            .query_row(
                "SELECT home_id FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("identity column added");
        assert!(
            home_id.is_none(),
            "identity stays unset until the binding writes it"
        );
    }

    /// The exact v5 Catalog shape (spec §3.4): the identity-bearing schema
    /// a pre-#48 build wrote. Used to pin the v5→v6 migration contract.
    fn seed_v5_catalog(path: &Path) {
        let connection = Connection::open(path).expect("create v5 catalog");
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
                    source_kind TEXT NOT NULL CHECK (source_kind IN ('link', 'remote_install', 'file_install')),
                    library_entry_path TEXT,
                    final_entity_path TEXT NOT NULL,
                    recorded_content_hash TEXT,
                    health TEXT NOT NULL CHECK (health IN ('healthy', 'broken', 'modified')),
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
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
                VALUES (1, 5, 0);
                INSERT INTO preferences (singleton) VALUES (1);
                "#,
            )
            .expect("create v5 schema");
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_v5_remote_skill(
        connection: &Connection,
        skill_id: &str,
        source_url: &str,
        requested_ref: &str,
        resolved_commit: &str,
        skill_path: &str,
        content_hash: Option<&str>,
        last_checked: Option<i64>,
    ) {
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (?1, ?1, ?1, ?1, '', 'remote_install', '/lib/' || ?1, '/lib/' || ?1, ?2, 'healthy', 'now', 'now')",
                params![skill_id, content_hash],
            )
            .expect("seed v5 remote skill");
        connection
            .execute(
                "INSERT INTO remote_sources (
                    skill_id, source_url, requested_ref, resolved_commit, skill_path,
                    last_checked_at, last_updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    skill_id,
                    source_url,
                    requested_ref,
                    resolved_commit,
                    skill_path,
                    last_checked
                ],
            )
            .expect("seed v5 remote source row");
    }

    #[test]
    fn v5_to_v6_migration_moves_remote_rows_into_parents_and_bindings() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("seed connection");
            // Two rows, same repository with different URL spellings: the
            // normalized identity must produce ONE parent, two bindings.
            seed_v5_remote_skill(
                &connection,
                "alpha",
                "https://github.com/acme/skills.git",
                "HEAD",
                "c0ffee0000000000000000000000000000000000",
                "skills/alpha",
                Some("tree-sha256-v1:aaaa"),
                Some(1700000000),
            );
            seed_v5_remote_skill(
                &connection,
                "beta",
                "https://github.com/acme/skills/",
                "refs/tags/v1.2.3",
                "deadbeef00000000000000000000000000000000",
                "",
                Some("tree-sha256-v1:bbbb"),
                None,
            );
            seed_v5_remote_skill(
                &connection,
                "gamma",
                "file:///Users/tester/work/local-skills",
                "main",
                "1111111111111111111111111111111111111111",
                "tools/gamma",
                Some("tree-sha256-v1:cccc"),
                None,
            );
        }
        let mut connection = Connection::open(&path).expect("reopen");
        migrate_to_current(&mut connection, 5).expect("v5 → current migration");

        let schema: u32 = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema version");
        assert_eq!(schema, CURRENT_SCHEMA_VERSION);

        // Integrity and foreign keys passed inside the migration transaction
        // before the old table was dropped; re-verify on the live file.
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity check");
        assert_eq!(integrity, "ok");
        let violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(violations, 0);

        // The old table is gone; the new tables exist.
        let remote_sources_present: bool = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'remote_sources'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count != 0),
            )
            .expect("old table query");
        assert!(!remote_sources_present, "remote_sources must be dropped");

        // Two distinct normalized parents (github.com/acme/skills and the
        // file URL); the two GitHub spellings share one parent.
        let mut parents = connection
            .prepare(
                "SELECT remote_id, canonical_url FROM remote_source_parents ORDER BY canonical_url",
            )
            .expect("parents query")
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .expect("parents rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("parents collect");
        assert_eq!(parents.len(), 2);
        parents.sort_by(|left, right| left.1.cmp(&right.1));
        assert_eq!(parents[0].1, "file:///Users/tester/work/local-skills");
        assert_eq!(parents[1].1, "https://github.com/acme/skills");

        let binding = |skill_id: &str| -> (
            String,
            String,
            bool,
            String,
            Option<String>,
            String,
            String,
            Option<i64>,
        ) {
            connection
                .query_row(
                    "SELECT b.requested_ref, b.verification_anchor_commit, b.original_commit_known,
                            b.skill_path, b.provider_hash, b.remote_baseline_hash,
                            b.current_baseline_hash, b.last_checked_at
                       FROM remote_bindings b WHERE b.skill_id = ?1",
                    [skill_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get::<_, bool>(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get::<_, Option<i64>>(7)?,
                        ))
                    },
                )
                .expect("binding row")
        };

        let (
            ref_alpha,
            anchor_alpha,
            known_alpha,
            path_alpha,
            hash_alpha,
            remote_base_alpha,
            current_base_alpha,
            checked_alpha,
        ) = binding("alpha");
        assert_eq!(ref_alpha, "HEAD");
        assert_eq!(anchor_alpha, "c0ffee0000000000000000000000000000000000");
        assert!(
            known_alpha,
            "legacy resolved commit is the known install commit"
        );
        assert_eq!(path_alpha, "skills/alpha");
        assert_eq!(
            hash_alpha, None,
            "provider hash stays NULL until a verified fetch"
        );
        assert_eq!(remote_base_alpha, "tree-sha256-v1:aaaa");
        assert_eq!(current_base_alpha, "tree-sha256-v1:aaaa");
        assert_eq!(
            checked_alpha,
            Some(1700000000),
            "last_checked_at is preserved"
        );

        let (
            ref_beta,
            anchor_beta,
            known_beta,
            path_beta,
            _,
            remote_base_beta,
            current_base_beta,
            _,
        ) = binding("beta");
        assert_eq!(ref_beta, "refs/tags/v1.2.3");
        assert_eq!(anchor_beta, "deadbeef00000000000000000000000000000000");
        assert!(known_beta);
        assert_eq!(path_beta, "");
        assert_eq!(remote_base_beta, "tree-sha256-v1:bbbb");
        assert_eq!(current_base_beta, "tree-sha256-v1:bbbb");

        let (_, _, _, _, _, _, _, _) = binding("gamma");
    }

    #[test]
    fn v5_to_v6_migration_fails_closed_on_missing_content_hash() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("seed connection");
            seed_v5_remote_skill(
                &connection,
                "alpha",
                "https://github.com/acme/skills.git",
                "HEAD",
                "c0ffee0000000000000000000000000000000000",
                "skills/alpha",
                None,
                None,
            );
        }
        let mut connection = Connection::open(&path).expect("reopen");
        let error = migrate_to_current(&mut connection, 5).expect_err("migration must fail");
        assert!(error.to_string().contains("no recorded content hash"));
        // The transaction rolled back: the v5 schema and rows are intact.
        let schema: u32 = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema version");
        assert_eq!(schema, 5);
        let remote_sources_present: bool = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'remote_sources'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count != 0),
            )
            .expect("old table query");
        assert!(
            remote_sources_present,
            "old table must survive a failed migration"
        );
    }

    #[test]
    fn v5_to_v6_migration_fails_closed_on_non_normalizable_url() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("seed connection");
            seed_v5_remote_skill(
                &connection,
                "alpha",
                "git@github.com:acme/skills.git",
                "HEAD",
                "c0ffee0000000000000000000000000000000000",
                "",
                Some("tree-sha256-v1:aaaa"),
                None,
            );
        }
        let mut connection = Connection::open(&path).expect("reopen");
        let error = migrate_to_current(&mut connection, 5).expect_err("migration must fail");
        assert!(error.to_string().contains("non-normalizable URL"));
        let schema: u32 = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema version");
        assert_eq!(schema, 5);
    }

    #[test]
    fn identity_bearing_v5_catalog_migrates_on_ordinary_open() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        {
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("seed connection");
            connection
                .execute(
                    "UPDATE catalog_meta
                     SET home_id = ?1, volume_fsid = ?2, volume_uuid = ?3, home_bound_at = ?4
                     WHERE singleton = 1",
                    params![
                        "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
                        "fsid",
                        "uuid",
                        "2026-08-01T00:00:00Z",
                    ],
                )
                .expect("record identity");
            seed_v5_remote_skill(
                &connection,
                "alpha",
                "https://github.com/acme/skills.git",
                "HEAD",
                "c0ffee0000000000000000000000000000000000",
                "",
                Some("tree-sha256-v1:aaaa"),
                None,
            );
        }
        let store = SqliteCatalogStore::open(&path).expect("ordinary open migrates v5 in place");
        assert_eq!(store.startup_status().access, StartupAccess::ReadWrite);
        assert_eq!(
            store.startup_status().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let remote_installs = store
            .load_remote_installs()
            .expect("migrated remote installs load");
        assert_eq!(remote_installs.len(), 1);
        assert_eq!(
            remote_installs[0].source_url,
            "https://github.com/acme/skills"
        );
        assert_eq!(
            remote_installs[0].verification_anchor_commit,
            "c0ffee0000000000000000000000000000000000"
        );
        assert!(remote_installs[0].original_commit_known);
        assert_eq!(remote_installs[0].provider_hash, None);
    }

    #[test]
    fn open_bound_migrates_identity_bearing_v5_in_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let bound = bound_home();
        {
            seed_v5_catalog(&path);
            let connection = Connection::open(&path).expect("seed connection");
            connection
                .execute(
                    "UPDATE catalog_meta
                     SET home_id = ?1, volume_fsid = ?2, volume_uuid = ?3, home_bound_at = ?4
                     WHERE singleton = 1",
                    params![
                        bound.home_id.0,
                        bound.volume_fsid,
                        bound.volume_uuid,
                        bound.bound_at
                    ],
                )
                .expect("record identity");
        }
        let store = SqliteCatalogStore::open_bound(&bound, &path).expect("open_bound migrates v5");
        assert_eq!(store.startup_status().access, StartupAccess::ReadWrite);
        assert_eq!(
            store.startup_status().schema_version,
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn create_bound_and_open_bound_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let bound = bound_home();
        SqliteCatalogStore::create_bound(&bound, &path).expect("create bound Catalog");

        let opened = SqliteCatalogStore::open_bound(&bound, &path).expect("reopen bound Catalog");
        assert_eq!(opened.startup_status().access, StartupAccess::ReadWrite);
        assert_eq!(
            opened.startup_status().schema_version,
            CURRENT_SCHEMA_VERSION
        );

        let probe = crate::adapters::catalog_probe::SqliteCatalogProbe::new();
        let report = probe.probe(&path).expect("read-only probe");
        assert!(report.exists);
        assert_eq!(report.schema_version, Some(CURRENT_SCHEMA_VERSION));
        assert!(report.integrity_ok);
        assert!(report.foreign_keys_ok);
        let identity = report.home_identity.expect("recorded identity");
        assert_eq!(identity.home_id, bound.home_id);
        assert_eq!(identity.volume_fsid, bound.volume_fsid);
        assert_eq!(identity.volume_uuid, bound.volume_uuid);
    }

    #[test]
    fn open_bound_rejects_missing_schema_and_identity_mismatch() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bound = bound_home();

        // Missing file.
        let missing = dir.path().join("missing.sqlite3");
        assert!(matches!(
            SqliteCatalogStore::open_bound(&bound, &missing),
            Err(BoundCatalogOpenError::Missing)
        ));

        // Fresh v5 without identity.
        let fresh = dir.path().join("fresh.sqlite3");
        SqliteCatalogStore::open(&fresh).expect("fresh open");
        assert!(matches!(
            SqliteCatalogStore::open_bound(&bound, &fresh),
            Err(BoundCatalogOpenError::IdentityMissing)
        ));

        // Bound to a different Home.
        let other = BoundHome::test_value(
            "c1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/other-home"),
        );
        let path = dir.path().join("other.sqlite3");
        SqliteCatalogStore::create_bound(&other, &path).expect("create other bound Catalog");
        assert!(matches!(
            SqliteCatalogStore::open_bound(&bound, &path),
            Err(BoundCatalogOpenError::IdentityMismatch)
        ));
    }

    #[test]
    fn create_bound_refuses_an_existing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        SqliteCatalogStore::open(&path).expect("fresh open");
        assert!(matches!(
            SqliteCatalogStore::create_bound(&bound_home(), &path),
            Err(BoundCatalogOpenError::AlreadyExists)
        ));
    }

    #[test]
    fn backup_catalog_produces_a_restorable_copy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        SqliteCatalogStore::open(&path).expect("fresh open");
        let backup = backup_catalog(&path, 4).expect("backup copy");
        let source = Connection::open(&path).expect("source");
        let mut destination = Connection::open(&backup).expect("backup connection");
        rusqlite::backup::Backup::new(&source, &mut destination)
            .and_then(|backup| backup.run_to_completion(128, Duration::from_millis(10), None))
            .expect("restore backup");
        let schema: u32 = destination
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema version");
        assert_eq!(schema, CURRENT_SCHEMA_VERSION);
    }
}
