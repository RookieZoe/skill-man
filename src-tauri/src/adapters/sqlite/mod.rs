use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentId, AgentKind, CatalogFilter, Health, SkillId, SkillSummary,
    SourceKind, agent_name_identity_key, configured_path_identity_key,
};
use crate::core::home::{BoundHome, HomeId};
use crate::seams::activation_store::{
    ActivationCellRow, ActivationCellWrite, ActivationObservation, ActivationStore,
    ActivationStoreError, DesiredActivation, StoredActivationObservation,
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
    SourcePromotionStore, SourcePromotionStoreError,
};
use crate::seams::source_transition_store::{
    ExistingSourceMember, SourceTransitionRecord, SourceTransitionStore, SourceTransitionStoreError,
};

mod agent_configuration;
mod prepared;
mod source_update_lifecycle;
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
    directory_identity_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    source_kind TEXT NOT NULL CHECK (source_kind IN ('link', 'remote_install', 'file_install')),
    library_entry_path TEXT,
    final_entity_path TEXT NOT NULL,
    recorded_content_hash TEXT,
    health TEXT NOT NULL CHECK (health IN ('healthy', 'broken', 'modified', 'source_snapshot_mismatch')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (source_kind = 'link' AND library_entry_path IS NULL)
        OR (source_kind != 'link' AND library_entry_path IS NOT NULL)
    )
);

CREATE INDEX skills_directory_identity
ON skills(directory_identity_key);

CREATE TABLE agent_configurations (
    agent_id TEXT PRIMARY KEY,
    origin TEXT NOT NULL CHECK (origin IN ('preset', 'custom')),
    preset_key TEXT,
    name TEXT NOT NULL,
    name_identity_key TEXT NOT NULL UNIQUE,
    compatibility TEXT NOT NULL CHECK (compatibility IN ('verified', 'unknown')),
    project_skills_dir TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (origin = 'preset' AND preset_key IS NOT NULL)
        OR (origin = 'custom' AND preset_key IS NULL)
    )
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
    role TEXT NOT NULL CHECK (role IN ('scan_only', 'activation_target')),
    PRIMARY KEY (agent_id, root_id)
);

CREATE UNIQUE INDEX one_activation_target_per_agent
ON agent_global_roots(agent_id)
WHERE role = 'activation_target';

CREATE TABLE activations (
    skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    target_root_id TEXT NOT NULL REFERENCES global_skill_roots(root_id) ON DELETE RESTRICT,
    directory_identity_key TEXT NOT NULL,
    desired_enabled INTEGER NOT NULL CHECK (desired_enabled IN (0, 1)),
    expected_entry_path TEXT NOT NULL,
    expected_target_path TEXT NOT NULL,
    observed_state TEXT NOT NULL CHECK (
        observed_state IN ('present', 'missing', 'target_mismatch', 'dangling', 'occupied')
    ),
    last_enabled_at TEXT,
    last_checked_at TEXT,
    PRIMARY KEY (skill_id, target_root_id)
);

CREATE UNIQUE INDEX active_activation_entry
ON activations(target_root_id, directory_identity_key)
WHERE desired_enabled = 1;

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
    tracking_mode TEXT NOT NULL CHECK (
        tracking_mode IN (
            'auto_release_tag_head', 'prerelease_channel', 'fixed_tag',
            'fixed_commit', 'branch', 'head'
        )
    ),
    tracking_value TEXT,
    current_selected_ref TEXT,
    current_release_id TEXT REFERENCES git_source_releases(release_id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (provider, canonical_url)
);

CREATE TABLE git_source_releases (
    release_id TEXT PRIMARY KEY,
    remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
    selection_kind TEXT NOT NULL,
    selected_ref TEXT NOT NULL,
    resolved_commit TEXT NOT NULL,
    discovered_at TEXT NOT NULL,
    UNIQUE (remote_id, selected_ref, resolved_commit)
);

CREATE TABLE git_source_release_members (
    release_id TEXT NOT NULL REFERENCES git_source_releases(release_id) ON DELETE CASCADE,
    skill_id TEXT NOT NULL REFERENCES skills(id),
    skill_path TEXT NOT NULL,
    directory_name TEXT NOT NULL,
    directory_identity_key TEXT NOT NULL,
    tree_hash TEXT NOT NULL,
    provider_hash TEXT,
    PRIMARY KEY (release_id, skill_path),
    UNIQUE (release_id, skill_id)
);

CREATE TABLE git_source_members (
    skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
    remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
    skill_path TEXT NOT NULL,
    storage_relpath TEXT NOT NULL,
    presence TEXT NOT NULL CHECK (presence IN ('current', 'absent')),
    first_seen_release_id TEXT NOT NULL REFERENCES git_source_releases(release_id),
    last_seen_release_id TEXT NOT NULL REFERENCES git_source_releases(release_id),
    last_checked_at INTEGER,
    last_updated_at INTEGER,
    UNIQUE (remote_id, skill_path),
    UNIQUE (remote_id, storage_relpath)
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
VALUES (1, 9, 0);

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

    /// Reopen an Existing Home Recovery Catalog without changing its durable
    /// SQLite state. Unlike `open_bound`, this deliberately neither selects a
    /// journal mode nor migrates: recovery owns only the missing app-level
    /// locator, never the recovered Home's Catalog. A non-current schema is
    /// rejected so the bootstrap/runtime boundary converges closed instead of
    /// silently upgrading user data.
    pub fn open_bound_without_catalog_mutation(
        bound: &BoundHome,
        path: &Path,
    ) -> Result<Self, BoundCatalogOpenError> {
        if !has_content(path) {
            return Err(BoundCatalogOpenError::Missing);
        }
        let schema = detect_schema_version(path);
        if schema != CURRENT_SCHEMA_VERSION {
            return Err(BoundCatalogOpenError::SchemaNotBound { found: schema });
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(BoundCatalogOpenError::Open)?;
        configure_connection_without_catalog_mutation(&connection)
            .map_err(BoundCatalogOpenError::Configure)?;
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

    pub fn skill_directory_identity_key(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<String>, CatalogStoreError> {
        self.connection()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))?
            .query_row(
                "SELECT directory_identity_key FROM skills WHERE id = ?1",
                [&skill_id.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| CatalogStoreError::Unavailable(error.to_string()))
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
                "SELECT target_root_id, expected_entry_path, expected_target_path
                 FROM activations
                 WHERE skill_id = ?1 AND desired_enabled = 1
                 ORDER BY target_root_id",
            )
            .map_err(sqlite_maintenance_error)?;
        statement
            .query_map([&skill_id.0], |row| {
                Ok(RelocateActivationBaseline {
                    target_root_id: row.get(0)?,
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
                     WHERE skill_id = ?1 AND target_root_id = ?2 AND desired_enabled = 1",
                    params![
                        skill_id.0,
                        activation.target_root_id,
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

    /// Git Source Member check (ADR-0018): a member of a Git Repository
    /// Source has no independent Remove.
    pub fn is_git_source_member(&self, skill_id: &SkillId) -> Result<bool, MaintenanceStoreError> {
        self.connection
            .lock()
            .map_err(|_| MaintenanceStoreError::Unavailable("SQLite lock poisoned".into()))?
            .query_row(
                "SELECT 1 FROM git_source_members WHERE skill_id = ?1",
                [&skill_id.0],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
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
                        id, directory_name, directory_identity_key, display_name, description,
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
                            skill_id, target_root_id, directory_identity_key, desired_enabled,
                            expected_entry_path, expected_target_path, observed_state,
                            last_enabled_at, last_checked_at
                         ) VALUES (?1, ?2, ?3, 1, ?4, ?5, 'present',
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                         ON CONFLICT(skill_id, target_root_id) DO NOTHING",
                        params![
                            record.skill.skill_id.0,
                            activation.target_root_id,
                            record.skill.identity_key,
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
        read_legacy_source_promotion(&connection, remote_id)
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
                "INSERT INTO git_source_releases (
                    release_id, remote_id, selection_kind, selected_ref, resolved_commit, discovered_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.release_id,
                    record.remote_id,
                    record.selection_kind,
                    record.selected_ref,
                    record.resolved_commit,
                ],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "INSERT INTO git_repository_sources (
                    remote_id, provider, canonical_url, tracking_mode, tracking_value,
                    current_selected_ref, current_release_id, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.remote_id,
                    record.provider,
                    record.canonical_url,
                    record.tracking_mode,
                    record.tracking_value,
                    record.selected_ref,
                    record.release_id,
                ],
            )
            .map_err(source_promotion_sql_error)?;

        for member in &record.members {
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
                                 health = 'healthy',
                                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             WHERE id = ?5",
                            params![
                                member.display_name,
                                member.description,
                                member.storage_relpath,
                                member.tree_hash,
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
                                id, directory_name, directory_identity_key, display_name, description,
                                source_kind, library_entry_path, final_entity_path,
                                recorded_content_hash, health, created_at, updated_at
                             ) VALUES (
                                ?1, ?2, ?3, ?4, ?5,
                                'remote_install', ?6, ?6, ?7, 'healthy',
                                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                                strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             )",
                            params![
                                member.skill_id.0,
                                member.directory_name,
                                member.identity_key,
                                member.display_name,
                                member.description,
                                member.storage_relpath,
                                member.tree_hash,
                            ],
                        )
                        .map_err(source_promotion_sql_error)?;
                }
            }
            transaction
                .execute(
                    "INSERT INTO git_source_release_members (
                        release_id, skill_id, skill_path, directory_name,
                        directory_identity_key, tree_hash, provider_hash
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        record.release_id,
                        member.skill_id.0,
                        member.skill_path,
                        member.directory_name,
                        member.identity_key,
                        member.tree_hash,
                        member.provider_hash,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
            transaction
                .execute(
                    "INSERT INTO git_source_members (
                        skill_id, remote_id, skill_path, storage_relpath, presence,
                        first_seen_release_id, last_seen_release_id, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, 'current', ?5, ?5, unixepoch('now'), unixepoch('now'))",
                    params![
                        member.skill_id.0,
                        record.remote_id,
                        member.skill_path,
                        member.storage_relpath,
                        record.release_id,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
        }

        for removed in &record.removed_members {
            let deleted = transaction
                .execute("DELETE FROM skills WHERE id = ?1", [&removed.skill_id.0])
                .map_err(source_promotion_sql_error)?;
            if deleted != 1 {
                return Err(SourcePromotionStoreError::Conflict(format!(
                    "Legacy removed member '{}' disappeared during Source Promotion",
                    removed.directory_name
                )));
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
            .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?;
        if !source_promotion_matches(&transaction, record)? {
            return Err(SourcePromotionStoreError::Conflict(
                "the Source Promotion no longer matches the frozen result".into(),
            ));
        }
        transaction
            .execute(
                "DELETE FROM git_source_release_members WHERE release_id = ?1",
                [&record.release_id],
            )
            .map_err(source_promotion_sql_error)?;
        // Drop every Git-member row for this source first (New + Legacy).
        let deleted = transaction
            .execute(
                "DELETE FROM git_source_members WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        if deleted != record.members.len() {
            return Err(SourcePromotionStoreError::Conflict(
                "the frozen Git member set is no longer complete".into(),
            ));
        }
        for member in &record.members {
            match member.origin {
                SourcePromotionMemberOrigin::Legacy => {
                    let entity = member.legacy_entity.as_ref().ok_or_else(|| {
                        SourcePromotionStoreError::Conflict(format!(
                            "Legacy member '{}' has no frozen audit entity",
                            member.directory_name
                        ))
                    })?;
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
                                entity.display_name,
                                entity.description,
                                entity.final_entity_path.to_string_lossy(),
                                entity.recorded_content_hash,
                                health_value(entity.health),
                                entity.skill_id.0,
                            ],
                        )
                        .map_err(source_promotion_sql_error)?;
                    if changed != 1 {
                        return Err(SourcePromotionStoreError::Conflict(format!(
                            "Legacy member '{}' disappeared during Source Promotion Undo",
                            member.directory_name
                        )));
                    }
                    transaction
                        .execute(
                            "INSERT INTO remote_bindings (
                                skill_id, remote_id, requested_ref, verification_anchor_commit,
                                original_commit_known, skill_path, provider_hash,
                                remote_baseline_hash, current_baseline_hash,
                                last_checked_at, last_updated_at
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                            params![
                                entity.skill_id.0,
                                record.remote_id,
                                entity.requested_ref,
                                entity.verification_anchor_commit,
                                entity.original_commit_known,
                                entity.skill_path,
                                entity.provider_hash,
                                entity.remote_baseline_hash,
                                entity.current_baseline_hash,
                                entity.last_checked_at,
                                entity.last_updated_at,
                            ],
                        )
                        .map_err(source_promotion_sql_error)?;
                }
                SourcePromotionMemberOrigin::New => {
                    let deleted = transaction
                        .execute("DELETE FROM skills WHERE id = ?1", [&member.skill_id.0])
                        .map_err(source_promotion_sql_error)?;
                    if deleted != 1 {
                        return Err(SourcePromotionStoreError::Conflict(format!(
                            "Source member '{}' disappeared during Source Promotion Undo",
                            member.directory_name
                        )));
                    }
                }
            }
        }
        for removed in &record.removed_members {
            let entity = &removed.legacy_entity;
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, directory_identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?6, ?7, ?8,
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        entity.skill_id.0,
                        entity.directory_name,
                        entity.identity_key,
                        entity.display_name,
                        entity.description,
                        entity.final_entity_path.to_string_lossy(),
                        entity.recorded_content_hash,
                        health_value(entity.health),
                    ],
                )
                .map_err(source_promotion_sql_error)?;
            transaction
                .execute(
                    "INSERT INTO remote_bindings (
                        skill_id, remote_id, requested_ref, verification_anchor_commit,
                        original_commit_known, skill_path, provider_hash,
                        remote_baseline_hash, current_baseline_hash, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        entity.skill_id.0,
                        record.remote_id,
                        entity.requested_ref,
                        entity.verification_anchor_commit,
                        entity.original_commit_known,
                        entity.skill_path,
                        entity.provider_hash,
                        entity.remote_baseline_hash,
                        entity.current_baseline_hash,
                        entity.last_checked_at,
                        entity.last_updated_at,
                    ],
                )
                .map_err(source_promotion_sql_error)?;
            for activation in &entity.activations {
                transaction
                    .execute(
                        "INSERT INTO activations (
                            skill_id, target_root_id, directory_identity_key, desired_enabled,
                            expected_entry_path, expected_target_path, observed_state,
                            last_enabled_at, last_checked_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            entity.skill_id.0,
                            activation.target_root_id,
                            crate::core::domain::skill_identity_key(&entity.directory_name),
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
        }
        transaction
            .execute(
                "DELETE FROM git_repository_sources WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        transaction
            .execute(
                "DELETE FROM git_source_releases WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(source_promotion_sql_error)?;
        let _ = legacy;
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
    fn existing_current_members(
        &self,
        canonical_url: &str,
    ) -> Result<Option<Vec<ExistingSourceMember>>, SourceTransitionStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let remote_id: Option<String> = connection
            .query_row(
                "SELECT remote_id FROM git_repository_sources WHERE canonical_url = ?1",
                [canonical_url],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        let Some(remote_id) = remote_id else {
            return Ok(None);
        };
        let members = connection
            .prepare(
                "SELECT git_source_members.skill_id, skills.directory_name,
                        git_source_members.skill_path
                 FROM git_source_members
                 JOIN skills ON skills.id = git_source_members.skill_id
                 WHERE git_source_members.remote_id = ?1 AND presence = 'current'
                 ORDER BY git_source_members.skill_path",
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
            .query_map([&remote_id], |row| {
                Ok(ExistingSourceMember {
                    skill_id: row.get(0)?,
                    directory_name: row.get(1)?,
                    skill_path: row.get(2)?,
                })
            })
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        Ok(Some(members))
    }

    fn existing_source(
        &self,
        remote_id: &str,
    ) -> Result<
        Option<crate::seams::source_transition_store::ExistingSourceFacts>,
        SourceTransitionStoreError,
    > {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceTransitionStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let canonical_url: Option<String> = connection
            .query_row(
                "SELECT canonical_url FROM git_repository_sources WHERE remote_id = ?1",
                [remote_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        let Some(canonical_url) = canonical_url else {
            return Ok(None);
        };
        let members = connection
            .prepare(
                "SELECT git_source_members.skill_id, skills.directory_name,
                        git_source_members.skill_path
                 FROM git_source_members
                 JOIN skills ON skills.id = git_source_members.skill_id
                 WHERE git_source_members.remote_id = ?1 AND presence = 'current'
                 ORDER BY git_source_members.skill_path",
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
            .query_map([remote_id], |row| {
                Ok(ExistingSourceMember {
                    skill_id: row.get(0)?,
                    directory_name: row.get(1)?,
                    skill_path: row.get(2)?,
                })
            })
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        Ok(Some(
            crate::seams::source_transition_store::ExistingSourceFacts {
                remote_id: remote_id.into(),
                canonical_url,
                members,
            },
        ))
    }

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
                     WHERE id = ?1
                        OR (
                            directory_identity_key = ?2
                            AND NOT EXISTS (
                                SELECT 1 FROM git_source_members
                                 WHERE git_source_members.skill_id = skills.id
                            )
                        )
                     LIMIT 1",
                    params![member.skill_id.0, member.identity_key],
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
                "INSERT INTO git_source_releases (
                    release_id, remote_id, selection_kind, selected_ref, resolved_commit, discovered_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.release_id,
                    record.remote_id,
                    record.selection_kind,
                    record.selected_ref,
                    record.resolved_commit,
                ],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .execute(
                "INSERT INTO git_repository_sources (
                    remote_id, provider, canonical_url, tracking_mode, tracking_value,
                    current_selected_ref, current_release_id, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.remote_id,
                    record.provider,
                    record.canonical_url,
                    record.tracking_mode,
                    record.tracking_value,
                    record.selected_ref,
                    record.release_id,
                ],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        for alias in &record.aliases {
            transaction
                .execute(
                    "INSERT INTO remote_source_aliases (remote_id, alias_url, confirmed_at)
                     VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![record.remote_id, alias],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        }
        for member in &record.members {
            transaction
                .execute(
                    "INSERT INTO skills (
                        id, directory_name, directory_identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5,
                        'remote_install', ?6, ?6, ?7, 'healthy',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     )",
                    params![
                        member.skill_id.0,
                        member.directory_name,
                        member.identity_key,
                        member.display_name,
                        member.description,
                        member.storage_relpath,
                        member.tree_hash,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO git_source_release_members (
                        release_id, skill_id, skill_path, directory_name,
                        directory_identity_key, tree_hash, provider_hash
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        record.release_id,
                        member.skill_id.0,
                        member.skill_path,
                        member.directory_name,
                        member.identity_key,
                        member.tree_hash,
                        member.provider_hash,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO git_source_members (
                        skill_id, remote_id, skill_path, storage_relpath, presence,
                        first_seen_release_id, last_seen_release_id, last_checked_at, last_updated_at
                     ) VALUES (?1, ?2, ?3, ?4, 'current', ?5, ?5, unixepoch('now'), unixepoch('now'))",
                    params![
                        member.skill_id.0,
                        record.remote_id,
                        member.skill_path,
                        member.storage_relpath,
                        record.release_id,
                    ],
                )
                .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
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
        transaction
            .execute(
                "DELETE FROM git_source_release_members WHERE release_id = ?1",
                [&record.release_id],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        for member in &record.members {
            let deleted = transaction
                .execute("DELETE FROM skills WHERE id = ?1", [&member.skill_id.0])
                .map_err(|error| {
                    SourceTransitionStoreError::Unavailable(format!(
                        "DELETE skills '{}': {error}",
                        member.directory_name
                    ))
                })?;
            if deleted != 1 {
                return Err(SourceTransitionStoreError::Conflict(format!(
                    "Managed Skill '{}' is no longer present",
                    member.directory_name
                )));
            }
        }
        transaction
            .execute(
                "DELETE FROM git_repository_sources WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
        transaction
            .execute(
                "DELETE FROM git_source_releases WHERE remote_id = ?1",
                [&record.remote_id],
            )
            .map_err(|error| {
                SourceTransitionStoreError::Unavailable(format!(
                    "DELETE git_source_releases: {error}"
                ))
            })?;
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

/// Read the Legacy Per-Skill Git State of one parent. It fails closed when:
/// the parent has no complete member set, one non-empty tracking ref, any
/// non-`remote_install` member, or missing/incomplete binding facts.
fn read_legacy_source_promotion(
    connection: &Connection,
    remote_id: &str,
) -> Result<LegacySourcePromotionRecord, SourcePromotionStoreError> {
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
            "SELECT skills.id, skills.directory_name, skills.directory_identity_key,
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
        .collect::<BTreeSet<_>>();
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
    let mut member_paths = BTreeSet::new();
    let mut member_ids = BTreeSet::new();
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
        .prepare("SELECT configured_path FROM global_skill_roots ORDER BY path_identity_key")
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
                    "SELECT target_root_id, expected_entry_path, expected_target_path,
                            desired_enabled, observed_state, last_enabled_at, last_checked_at
                     FROM activations
                     WHERE skill_id = ?1
                     ORDER BY target_root_id",
                )
                .map_err(|error| SourcePromotionStoreError::Unavailable(error.to_string()))?
                .query_map([&member.skill_id.0], |row| {
                    Ok(SourcePromotionActivationRecord {
                        target_root_id: row.get(0)?,
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
        current_release_id: None,
        members,
        forbidden_local_link_roots,
    })
}

type SourceFactsRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

/// Exact, source-level current-release test for Promotion recovery and
/// Source Undo. It compares only the Git Repository Source facts; frozen
/// Legacy binding facts remain journal/audit evidence.
fn source_promotion_matches(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<bool, SourcePromotionStoreError> {
    let source: Option<SourceFactsRow> = connection
        .query_row(
            "SELECT remote_id, provider, tracking_mode, tracking_value,
                        current_selected_ref, current_release_id
                 FROM git_repository_sources WHERE remote_id = ?1",
            [&record.remote_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    let Some((remote_id, provider, tracking_mode, tracking_value, selected_ref, release_id)) =
        source
    else {
        return Ok(false);
    };
    if remote_id != record.remote_id
        || provider != record.provider
        || tracking_mode != record.tracking_mode
        || tracking_value != record.tracking_value
        || selected_ref != record.selected_ref
        || release_id != Some(record.release_id.clone())
    {
        return Ok(false);
    }
    let release: Option<(String, String)> = connection
        .query_row(
            "SELECT selection_kind, resolved_commit
             FROM git_source_releases WHERE release_id = ?1 AND remote_id = ?2",
            params![record.release_id, record.remote_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    if release
        != Some((
            record.selection_kind.clone(),
            record.resolved_commit.clone(),
        ))
    {
        return Ok(false);
    }
    if !release_members_match(connection, record)? {
        return Ok(false);
    }
    if !current_promotion_members_match(connection, record)? {
        return Ok(false);
    }
    for removed in &record.removed_members {
        let gone: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM skills WHERE id = ?1)",
                [&removed.skill_id.0],
                |row| row.get(0),
            )
            .map_err(source_promotion_sql_error)?;
        if gone {
            return Ok(false);
        }
    }
    Ok(true)
}

fn release_members_match(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<bool, SourcePromotionStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT skill_id, skill_path, tree_hash
             FROM git_source_release_members WHERE release_id = ?1
             ORDER BY skill_path, skill_id",
        )
        .map_err(source_promotion_sql_error)?;
    let actual = statement
        .query_map([&record.release_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(source_promotion_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(source_promotion_sql_error)?;
    let mut expected = record
        .members
        .iter()
        .map(|member| {
            (
                member.skill_id.0.clone(),
                member.skill_path.clone(),
                member.tree_hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| {
        (left.1.as_str(), left.0.as_str()).cmp(&(right.1.as_str(), right.0.as_str()))
    });
    Ok(actual == expected)
}

fn current_promotion_members_match(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<bool, SourcePromotionStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT skill_id, skill_path, storage_relpath, presence
             FROM git_source_members WHERE remote_id = ?1
             ORDER BY skill_path, skill_id",
        )
        .map_err(source_promotion_sql_error)?;
    let actual = statement
        .query_map([&record.remote_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(source_promotion_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(source_promotion_sql_error)?;
    let mut expected = record
        .members
        .iter()
        .map(|member| {
            (
                member.skill_id.0.clone(),
                member.skill_path.clone(),
                member.storage_relpath.clone(),
                "current".into(),
            )
        })
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| {
        (left.1.as_str(), left.0.as_str()).cmp(&(right.1.as_str(), right.0.as_str()))
    });
    Ok(actual == expected)
}

/// Pre-flight for one whole-source Promotion: proves the Legacy parent
/// still has its exact frozen binding set, that nothing of the v9 source
/// exists yet, and that no new member collides with the Library.
fn validate_source_promotion_record(
    connection: &Connection,
    record: &SourcePromotionRecord,
) -> Result<(), SourcePromotionStoreError> {
    if record.members.is_empty() {
        return Err(SourcePromotionStoreError::Conflict(
            "a Source Promotion must contain the complete target release".into(),
        ));
    }
    let parent: Option<()> = connection
        .query_row(
            "SELECT 1 FROM remote_source_parents WHERE remote_id = ?1 AND canonical_url = ?2",
            params![record.remote_id, record.canonical_url],
            |_| Ok(()),
        )
        .optional()
        .map_err(source_promotion_sql_error)?;
    let Some(()) = parent else {
        return Err(SourcePromotionStoreError::Conflict(
            "the Legacy parent no longer exists".into(),
        ));
    };
    let promoted: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM git_repository_sources WHERE remote_id = ?1)",
            [&record.remote_id],
            |row| row.get(0),
        )
        .map_err(source_promotion_sql_error)?;
    if promoted {
        return Err(SourcePromotionStoreError::Conflict(
            "the Legacy parent is already a Git Repository Source".into(),
        ));
    }
    let mut binding_statement = connection
        .prepare(
            "SELECT skills.id, remote_bindings.requested_ref,
                    remote_bindings.verification_anchor_commit,
                    remote_bindings.original_commit_known,
                    remote_bindings.skill_path,
                    remote_bindings.current_baseline_hash
             FROM remote_bindings
             JOIN skills ON skills.id = remote_bindings.skill_id
             WHERE remote_bindings.remote_id = ?1
             ORDER BY remote_bindings.skill_path, skills.id",
        )
        .map_err(source_promotion_sql_error)?;
    let bindings = binding_statement
        .query_map([&record.remote_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(source_promotion_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(source_promotion_sql_error)?;
    let mut bindings = bindings;
    bindings.sort_by(|left, right| {
        (left.4.as_str(), left.0.as_str()).cmp(&(right.4.as_str(), right.0.as_str()))
    });
    let mut expected_bindings = record
        .members
        .iter()
        .filter(|member| member.origin == SourcePromotionMemberOrigin::Legacy)
        .filter_map(|member| member.legacy_entity.as_ref())
        .chain(
            record
                .removed_members
                .iter()
                .map(|removed| &removed.legacy_entity),
        )
        .map(|entity| {
            (
                entity.skill_id.0.clone(),
                entity.requested_ref.clone(),
                entity.verification_anchor_commit.clone(),
                entity.original_commit_known,
                entity.skill_path.clone(),
                entity.current_baseline_hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    expected_bindings.sort_by(|left, right| {
        (left.4.as_str(), left.0.as_str()).cmp(&(right.4.as_str(), right.0.as_str()))
    });
    if bindings != expected_bindings {
        return Err(SourcePromotionStoreError::Conflict(
            "the frozen Legacy binding set changed during Source Promotion".into(),
        ));
    }
    if record
        .legacy_member_ids
        .iter()
        .any(|id| !bindings.iter().any(|binding| binding.0 == id.0))
    {
        return Err(SourcePromotionStoreError::Conflict(
            "the frozen Legacy member set is incomplete".into(),
        ));
    }
    let mut seen_ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    for member in &record.members {
        if !seen_ids.insert(member.skill_id.0.as_str())
            || !seen_paths.insert(member.skill_path.as_str())
        {
            return Err(SourcePromotionStoreError::Conflict(
                "the target release carries duplicate members".into(),
            ));
        }
        if member.origin == SourcePromotionMemberOrigin::New {
            let collision: Option<String> = connection
                .query_row(
                    "SELECT directory_name FROM skills
                     WHERE id = ?1
                        OR (
                            directory_identity_key = ?2
                            AND NOT EXISTS (
                                SELECT 1 FROM git_source_members
                                 WHERE git_source_members.skill_id = skills.id
                            )
                            AND NOT EXISTS (
                                SELECT 1 FROM remote_bindings
                                 WHERE remote_bindings.skill_id = skills.id
                                   AND remote_bindings.remote_id = ?3
                            )
                        )
                     LIMIT 1",
                    params![member.skill_id.0, member.identity_key, record.remote_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(source_promotion_sql_error)?;
            if let Some(directory_name) = collision {
                return Err(SourcePromotionStoreError::Conflict(format!(
                    "the Library already contains Managed Skill '{directory_name}'"
                )));
            }
        }
    }
    Ok(())
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
    for member in &record.members {
        if member.tree_hash.is_empty() || !member.storage_relpath.starts_with("skills/git/") {
            return Err(SourceTransitionStoreError::Conflict(
                "a Source Transition member has invalid frozen facts".into(),
            ));
        }
    }
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_source_parents WHERE canonical_url = ?1)",
            [&record.canonical_url],
            |row| row.get(0),
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    if exists {
        return Err(SourceTransitionStoreError::Conflict(format!(
            "the canonical repository '{}' already has a source parent",
            record.canonical_url
        )));
    }
    let mut seen_ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    for member in &record.members {
        if !seen_ids.insert(member.skill_id.0.as_str())
            || !seen_paths.insert(member.skill_path.as_str())
        {
            return Err(SourceTransitionStoreError::Conflict(
                "the source release carries duplicate members".into(),
            ));
        }
        let existing: Option<String> = connection
            .query_row(
                "SELECT directory_name FROM skills
                 WHERE id = ?1
                    OR (
                        directory_identity_key = ?2
                        AND NOT EXISTS (
                            SELECT 1 FROM git_source_members
                             WHERE git_source_members.skill_id = skills.id
                        )
                    )
                 LIMIT 1",
                params![member.skill_id.0, member.identity_key],
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

/// Exact, source-level state check shared by idempotent post-CAS recovery
/// and Source Undo: the source row, its current immutable release and every
/// frozen current member must still match the record exactly.
fn source_transition_matches(
    connection: &Connection,
    record: &SourceTransitionRecord,
) -> Result<bool, SourceTransitionStoreError> {
    let source: Option<SourceFactsRow> = connection
        .query_row(
            "SELECT remote_id, provider, tracking_mode, tracking_value,
                    current_selected_ref, current_release_id
             FROM git_repository_sources WHERE remote_id = ?1 AND canonical_url = ?2",
            params![record.remote_id, record.canonical_url],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let Some((remote_id, provider, tracking_mode, tracking_value, selected_ref, release_id)) =
        source
    else {
        return Ok(false);
    };
    if remote_id != record.remote_id
        || provider != record.provider
        || tracking_mode != record.tracking_mode
        || tracking_value != record.tracking_value
        || selected_ref != record.selected_ref
        || release_id != Some(record.release_id.clone())
    {
        return Ok(false);
    }
    let release: Option<(String, String)> = connection
        .query_row(
            "SELECT selection_kind, resolved_commit
             FROM git_source_releases WHERE release_id = ?1 AND remote_id = ?2",
            params![record.release_id, record.remote_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    if release
        != Some((
            record.selection_kind.clone(),
            record.resolved_commit.clone(),
        ))
    {
        return Ok(false);
    }
    let mut release_statement = connection
        .prepare(
            "SELECT skill_id, skill_path, tree_hash
             FROM git_source_release_members WHERE release_id = ?1
             ORDER BY skill_path, skill_id",
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let actual_release_members = release_statement
        .query_map([&record.release_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let mut expected_release_members = record
        .members
        .iter()
        .map(|member| {
            (
                member.skill_id.0.clone(),
                member.skill_path.clone(),
                member.tree_hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    expected_release_members.sort_by(|left, right| {
        (left.1.as_str(), left.0.as_str()).cmp(&(right.1.as_str(), right.0.as_str()))
    });
    if actual_release_members != expected_release_members {
        return Ok(false);
    }
    let mut member_statement = connection
        .prepare(
            "SELECT skill_id, skill_path, storage_relpath, presence
             FROM git_source_members WHERE remote_id = ?1
             ORDER BY skill_path, skill_id",
        )
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let actual_members = member_statement
        .query_map([&record.remote_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| SourceTransitionStoreError::Unavailable(error.to_string()))?;
    let mut expected_members = record
        .members
        .iter()
        .map(|member| {
            (
                member.skill_id.0.clone(),
                member.skill_path.clone(),
                format!("skills/git/{}/{}", record.remote_id, member.skill_id.0),
                "current".into(),
            )
        })
        .collect::<Vec<_>>();
    expected_members.sort_by(|left, right| {
        (left.1.as_str(), left.0.as_str()).cmp(&(right.1.as_str(), right.0.as_str()))
    });
    Ok(actual_members == expected_members)
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
                "SELECT id, directory_name FROM skills WHERE directory_identity_key = ?1",
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
                "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                    id, directory_name, directory_identity_key, display_name, description,
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
                "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                    id, directory_name, directory_identity_key, display_name, description,
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
                    "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                        id, directory_name, directory_identity_key, display_name, description,
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
                "SELECT skills.id, skills.directory_name, skills.directory_identity_key,
                        skills.display_name, skills.description,
                        skills.library_entry_path, skills.final_entity_path,
                        skills.recorded_content_hash, file_sources.original_path,
                        file_sources.original_filename
                   FROM skills
                   JOIN file_sources ON file_sources.skill_id = skills.id
                  WHERE skills.directory_identity_key = ?1 AND skills.source_kind = 'file_install'",
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
                  WHERE id = ?1 AND directory_identity_key = ?5 AND source_kind = 'file_install'",
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
                    "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                        id, directory_name, directory_identity_key, display_name, description,
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
                "SELECT skills.id, skills.directory_name, skills.directory_identity_key,
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
                "SELECT skills.id, skills.directory_name, skills.directory_identity_key,
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
                  WHERE skills.directory_identity_key = ?1 AND skills.source_kind = 'remote_install'",
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
                  WHERE id = ?1 AND directory_identity_key = ?5 AND source_kind = 'remote_install'",
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
        expected_requested_ref: &str,
        expected_verification_anchor: &str,
        requested_ref: &str,
    ) -> Result<(), ImportStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ImportStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_import_error)?;
        let changed = transaction
            .execute(
                "UPDATE remote_bindings
                    SET requested_ref = ?4
                  WHERE skill_id = ?1
                    AND requested_ref = ?2
                    AND verification_anchor_commit = ?3",
                params![
                    skill_id.0,
                    expected_requested_ref,
                    expected_verification_anchor,
                    requested_ref,
                ],
            )
            .map_err(sqlite_import_error)?;
        if changed != 1 {
            return Err(ImportStoreError::Stale(format!(
                "Skill '{}' no longer matches its pin preview",
                skill_id.0
            )));
        }
        transaction
            .execute(
                "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_import_error)?;
        transaction.commit().map_err(sqlite_import_error)?;
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
    fn snapshot_version(&self) -> u64 {
        let connection = match self.connection.lock() {
            Ok(connection) => connection,
            Err(_) => return 0,
        };
        let value: i64 = connection
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        u64::try_from(value).unwrap_or(0)
    }

    fn list_agents(&self) -> Result<Vec<AdoptAgent>, AdoptStoreError> {
        self.connection
            .lock()
            .map_err(|_| AdoptStoreError::Unavailable("SQLite lock poisoned".into()))?
            .prepare(
                "SELECT
                    configurations.agent_id,
                    configurations.name,
                    configurations.origin,
                    configurations.preset_key,
                    roots.root_id,
                    roots.configured_path,
                    memberships.role
                 FROM agent_configurations configurations
                 JOIN agent_global_roots memberships
                   ON memberships.agent_id = configurations.agent_id
                 JOIN global_skill_roots roots
                   ON roots.root_id = memberships.root_id
                 ORDER BY configurations.name_identity_key, roots.path_identity_key",
            )
            .map_err(sqlite_adopt_error)?
            .query_map([], |row| {
                let origin = row.get::<_, String>(2)?;
                let preset_key = row.get::<_, Option<String>>(3)?;
                Ok(AdoptAgent {
                    agent_id: AgentId(row.get(0)?),
                    root_id: row.get(4)?,
                    name: row.get(1)?,
                    kind: match (origin.as_str(), preset_key.as_deref()) {
                        ("preset", Some("claude-code")) => AgentKind::ClaudePreset,
                        ("preset", Some("codex")) => AgentKind::CodexPreset,
                        _ => AgentKind::Custom,
                    },
                    skills_path: PathBuf::from(row.get::<_, String>(5)?),
                    activation_target: row.get::<_, String>(6)? == "activation_target",
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
                "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                    id, directory_name, directory_identity_key, display_name, description,
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
                        skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at, last_checked_at
                     ) VALUES (?1, ?2, ?3, 1, ?4, ?5, 'present',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        record.skill_id.0,
                        activation.target_root_id,
                        record.identity_key,
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
                "SELECT id, directory_name, final_entity_path FROM skills WHERE directory_identity_key = ?1",
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
                "SELECT directory_name FROM skills WHERE directory_identity_key = ?1",
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
                    id, directory_name, directory_identity_key, display_name, description,
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
                        skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at, last_checked_at
                     ) VALUES (?1, ?2, ?3, 1, ?4, ?5, 'present',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        record.skill_id.0,
                        activation.target_root_id,
                        record.identity_key,
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
    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT skill_id, target_root_id, expected_entry_path, expected_target_path
                 FROM activations
                 WHERE desired_enabled = 1
                 ORDER BY skill_id, target_root_id",
            )
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([], |row| {
                Ok(DesiredActivation {
                    skill_id: SkillId(row.get(0)?),
                    target_root_id: row.get(1)?,
                    expected_entry_path: PathBuf::from(row.get::<_, String>(2)?),
                    expected_target_path: PathBuf::from(row.get::<_, String>(3)?),
                })
            })
            .map_err(sqlite_activation_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_activation_error)
    }

    fn activation_observations(
        &self,
    ) -> Result<Vec<StoredActivationObservation>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT skill_id, target_root_id, expected_entry_path, expected_target_path,
                        observed_state, last_checked_at
                 FROM activations
                 WHERE desired_enabled = 1
                 ORDER BY target_root_id, skill_id",
            )
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([], |row| {
                let observed_state: Option<String> = row.get(4)?;
                let last_checked_at: Option<String> = row.get(5)?;
                Ok(StoredActivationObservation {
                    skill_id: SkillId(row.get(0)?),
                    target_root_id: row.get(1)?,
                    expected_entry_path: PathBuf::from(row.get::<_, String>(2)?),
                    expected_target_path: PathBuf::from(row.get::<_, String>(3)?),
                    observed_state: observed_state
                        .as_deref()
                        .map(parse_observed_state)
                        .transpose()?,
                    last_checked_at_ms: last_checked_at.as_deref().and_then(parse_checked_at_ms),
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
                     WHERE skill_id = ?3 AND target_root_id = ?4 AND desired_enabled = 1",
                    params![
                        observed_state_value(observation.observed_state),
                        checked_at,
                        observation.skill_id.0,
                        observation.target_root_id,
                    ],
                )
                .map_err(sqlite_activation_error)?;
        }
        transaction
            .execute(
                "UPDATE catalog_meta
                 SET last_startup_check_at = ?1
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

    fn activation_cells(&self) -> Result<Vec<ActivationCellRow>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at
                 FROM activations
                 ORDER BY skill_id, target_root_id",
            )
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([], map_activation_cell_row)
            .map_err(sqlite_activation_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_activation_error)
    }

    fn activation_cells_for_skill(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<ActivationCellRow>, ActivationStoreError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at
                 FROM activations
                 WHERE skill_id = ?1
                 ORDER BY target_root_id",
            )
            .map_err(sqlite_activation_error)?;
        statement
            .query_map([&skill_id.0], map_activation_cell_row)
            .map_err(sqlite_activation_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_activation_error)
    }

    fn write_activation_cells(
        &self,
        writes: &[ActivationCellWrite],
    ) -> Result<u64, ActivationStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_activation_error)?;
        let now = unix_timestamp();
        for write in writes {
            transaction
                .execute(
                    "INSERT INTO activations (
                        skill_id, target_root_id, directory_identity_key, desired_enabled,
                        expected_entry_path, expected_target_path, observed_state,
                        last_enabled_at, last_checked_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'missing',
                        CASE WHEN ?4 = 1 THEN ?7 ELSE NULL END, NULL)
                     ON CONFLICT(skill_id, target_root_id) DO UPDATE SET
                        directory_identity_key = excluded.directory_identity_key,
                        desired_enabled = excluded.desired_enabled,
                        expected_entry_path = excluded.expected_entry_path,
                        expected_target_path = excluded.expected_target_path,
                        observed_state = CASE
                            WHEN excluded.desired_enabled = 1 THEN activations.observed_state
                            ELSE 'missing'
                        END,
                        last_enabled_at = CASE
                            WHEN excluded.desired_enabled = 1
                             AND activations.desired_enabled = 0
                            THEN excluded.last_enabled_at
                            ELSE activations.last_enabled_at
                        END,
                        last_checked_at = CASE
                            WHEN excluded.desired_enabled = 1
                             AND activations.desired_enabled = 0
                            THEN NULL
                            ELSE activations.last_checked_at
                        END",
                    params![
                        write.skill_id.0,
                        write.target_root_id,
                        write.directory_identity_key,
                        write.desired_enabled as i64,
                        write.expected_entry_path.to_string_lossy(),
                        write.expected_target_path.to_string_lossy(),
                        now,
                    ],
                )
                .map_err(|error| map_activation_cell_constraint(write, error))?;
        }
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

    fn catalog_generation(&self) -> Result<u64, ActivationStoreError> {
        let connection = self.connection()?;
        let value: i64 = connection
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
}

fn map_activation_cell_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActivationCellRow> {
    let observed_state: Option<String> = row.get(6)?;
    let last_enabled_at: Option<String> = row.get(7)?;
    Ok(ActivationCellRow {
        skill_id: SkillId(row.get(0)?),
        target_root_id: row.get(1)?,
        directory_identity_key: row.get(2)?,
        desired_enabled: row.get::<_, i64>(3)? != 0,
        expected_entry_path: PathBuf::from(row.get::<_, String>(4)?),
        expected_target_path: PathBuf::from(row.get::<_, String>(5)?),
        observed_state: observed_state
            .as_deref()
            .map(parse_observed_state)
            .transpose()?,
        last_enabled_at_ms: last_enabled_at.as_deref().and_then(parse_checked_at_ms),
    })
}

fn map_activation_cell_constraint(
    write: &ActivationCellWrite,
    error: rusqlite::Error,
) -> ActivationStoreError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) {
        return ActivationStoreError::EntryConflict {
            destination: write.expected_entry_path.to_string_lossy().into_owned(),
        };
    }
    sqlite_activation_error(error)
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

/// Per-connection settings for a recovered existing Home. `foreign_keys` and
/// the busy timeout are connection-local; unlike `journal_mode`, they do not
/// write the Catalog or create/update WAL sidecars.
fn configure_connection_without_catalog_mutation(
    connection: &Connection,
) -> Result<(), CatalogStoreOpenError> {
    connection
        .pragma_update(None, "foreign_keys", true)
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
    // The v9 step (spec §3.4) follows SQLite's documented table-rebuild
    // procedure: `foreign_keys` must be OFF for the `skills` rebuild, since
    // an enforced DROP TABLE would cascade into every child table. The step
    // itself still validates with `PRAGMA foreign_key_check` and
    // `PRAGMA integrity_check` inside the transaction; enforcement is
    // restored as soon as the migration transaction has ended.
    let rebuilds_skills = matches!(existing_schema_version, 4..=8);
    if rebuilds_skills {
        connection.pragma_update(None, "foreign_keys", false)?;
    }
    let outcome = migrate_to_current_inner(connection, existing_schema_version);
    if rebuilds_skills {
        let _ = connection.pragma_update(None, "foreign_keys", true);
    }
    outcome
}

fn migrate_to_current_inner(
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
    if matches!(existing_schema_version, 4..=7) {
        // Schema v8: Agent Configuration, shared Global Skills Roots and
        // Target-scoped Activations (spec §3.4, ADR-0016). Every validation,
        // copy, integrity check and legacy-table drop stays inside this
        // transaction. Any ambiguous path/name/Target aggregation therefore
        // leaves the complete v7 shape untouched.
        migrate_legacy_agents_to_configurations(&transaction)?;
    }
    if matches!(existing_schema_version, 4..=8) {
        // Schema v9 (spec §3.4, ADR-0018): rebuild `skills` so Directory
        // Identity is an ordinary indexed comparison key (stable `skill_id`
        // is the identity), and create the ADR-0018 Git namespace contract.
        migrate_to_v9(&transaction)?;
    }
    transaction.commit()
}

/// Schema v9 (ADR-0018, spec §3.4): rebuild `skills` (Directory Identity
/// constraint, stable `skill_id`, `source_snapshot_mismatch` closed state)
/// and move the four v7 Git tables to the v9 namespace contract. Existing
/// v7 Git facts are preserved verbatim under `legacy_git_*` names — they are
/// never guessed into the current model — and the v9 tables start empty.
/// Any failure in the transaction rolls the complete step back.
fn migrate_to_v9(transaction: &rusqlite::Transaction) -> rusqlite::Result<()> {
    // The pre-v9 `skills` column is `identity_key`; catalogs created by a
    // build that already began the rewind may carry the v9 name. The
    // Directory Identity value is copied either way — the constraint, not
    // the column spelling, is the contract.
    let identity_key = if table_has_column(transaction, "skills", "directory_identity_key")? {
        "directory_identity_key"
    } else {
        "identity_key"
    };
    transaction.execute_batch(
        "CREATE TABLE skills_v9 (
            id TEXT PRIMARY KEY,
            directory_name TEXT NOT NULL,
            directory_identity_key TEXT NOT NULL,
            display_name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            source_kind TEXT NOT NULL CHECK (source_kind IN ('link', 'remote_install', 'file_install')),
            library_entry_path TEXT,
            final_entity_path TEXT NOT NULL,
            recorded_content_hash TEXT,
            health TEXT NOT NULL CHECK (health IN ('healthy', 'broken', 'modified', 'source_snapshot_mismatch')),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            CHECK (
                (source_kind = 'link' AND library_entry_path IS NULL)
                OR (source_kind != 'link' AND library_entry_path IS NOT NULL)
            )
        );",
    )?;
    transaction.execute_batch(&format!(
        "INSERT INTO skills_v9 (
                id, directory_name, directory_identity_key, display_name, description,
                source_kind, library_entry_path, final_entity_path, recorded_content_hash,
                health, created_at, updated_at
            )
            SELECT id, directory_name, {identity_key}, display_name, description,
                   source_kind, library_entry_path, final_entity_path, recorded_content_hash,
                   health, created_at, updated_at
            FROM skills;
            DROP TABLE skills;
            ALTER TABLE skills_v9 RENAME TO skills;
            CREATE INDEX skills_directory_identity ON skills(directory_identity_key);",
    ))?;
    for (name, legacy_name) in [
        ("git_repository_sources", "legacy_git_repository_sources"),
        ("git_source_releases", "legacy_git_source_releases"),
        (
            "git_source_release_members",
            "legacy_git_source_release_members",
        ),
        ("git_source_members", "legacy_git_source_members"),
    ] {
        if table_exists(transaction, name)? {
            transaction.execute(&format!("ALTER TABLE {name} RENAME TO {legacy_name}"), [])?;
        }
    }
    transaction.execute_batch(
        "CREATE TABLE git_repository_sources (
            remote_id TEXT PRIMARY KEY REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            canonical_url TEXT NOT NULL,
            tracking_mode TEXT NOT NULL CHECK (
                tracking_mode IN (
                    'auto_release_tag_head', 'prerelease_channel', 'fixed_tag',
                    'fixed_commit', 'branch', 'head'
                )
            ),
            tracking_value TEXT,
            current_selected_ref TEXT,
            current_release_id TEXT REFERENCES git_source_releases(release_id),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (provider, canonical_url)
        );
        CREATE TABLE git_source_releases (
            release_id TEXT PRIMARY KEY,
            remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
            selection_kind TEXT NOT NULL,
            selected_ref TEXT NOT NULL,
            resolved_commit TEXT NOT NULL,
            discovered_at TEXT NOT NULL,
            UNIQUE (remote_id, selected_ref, resolved_commit)
        );
        CREATE TABLE git_source_release_members (
            release_id TEXT NOT NULL REFERENCES git_source_releases(release_id) ON DELETE CASCADE,
            skill_id TEXT NOT NULL REFERENCES skills(id),
            skill_path TEXT NOT NULL,
            directory_name TEXT NOT NULL,
            directory_identity_key TEXT NOT NULL,
            tree_hash TEXT NOT NULL,
            provider_hash TEXT,
            PRIMARY KEY (release_id, skill_path),
            UNIQUE (release_id, skill_id)
        );
        CREATE TABLE git_source_members (
            skill_id TEXT PRIMARY KEY REFERENCES skills(id) ON DELETE CASCADE,
            remote_id TEXT NOT NULL REFERENCES remote_source_parents(remote_id) ON DELETE CASCADE,
            skill_path TEXT NOT NULL,
            storage_relpath TEXT NOT NULL,
            presence TEXT NOT NULL CHECK (presence IN ('current', 'absent')),
            first_seen_release_id TEXT NOT NULL REFERENCES git_source_releases(release_id),
            last_seen_release_id TEXT NOT NULL REFERENCES git_source_releases(release_id),
            last_checked_at INTEGER,
            last_updated_at INTEGER,
            UNIQUE (remote_id, skill_path),
            UNIQUE (remote_id, storage_relpath)
        );
        UPDATE catalog_meta SET schema_version = 9 WHERE singleton = 1;",
    )?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
            Some(format!(
                "schema v9 migration left {violations} foreign-key violations"
            )),
        ));
    }
    let integrity: String =
        transaction.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("schema v9 migration failed the integrity check".into()),
        ));
    }
    Ok(())
}

fn table_exists(connection: &rusqlite::Connection, name: &str) -> rusqlite::Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    Ok(count != 0)
}

fn table_has_column(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> rusqlite::Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |row| row.get(0),
    )?;
    Ok(count != 0)
}

#[derive(Clone, Debug)]
struct LegacyAgentMigrationRow {
    agent_id: String,
    origin: &'static str,
    preset_key: Option<&'static str>,
    name: String,
    name_identity_key: String,
    compatibility: String,
    path_identity_key: String,
    created_at: String,
    updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyActivationAggregate {
    skill_id: String,
    root_id: String,
    directory_identity_key: String,
    desired_enabled: bool,
    expected_entry_path: String,
    expected_target_path: String,
    observed_state: String,
    last_enabled_at: Option<String>,
    last_checked_at: Option<String>,
}

fn migrate_legacy_agents_to_configurations(
    transaction: &rusqlite::Transaction,
) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "ALTER TABLE agents RENAME TO legacy_agents;
         ALTER TABLE activations RENAME TO legacy_activations;

         CREATE TABLE agent_configurations (
            agent_id TEXT PRIMARY KEY,
            origin TEXT NOT NULL CHECK (origin IN ('preset', 'custom')),
            preset_key TEXT,
            name TEXT NOT NULL,
            name_identity_key TEXT NOT NULL UNIQUE,
            compatibility TEXT NOT NULL CHECK (compatibility IN ('verified', 'unknown')),
            project_skills_dir TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            CHECK (
                (origin = 'preset' AND preset_key IS NOT NULL)
                OR (origin = 'custom' AND preset_key IS NULL)
            )
         );
         CREATE TABLE global_skill_roots (
            root_id TEXT PRIMARY KEY,
            configured_path TEXT NOT NULL,
            path_identity_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
         );
         CREATE TABLE agent_global_roots (
            agent_id TEXT NOT NULL
                REFERENCES agent_configurations(agent_id) ON DELETE CASCADE,
            root_id TEXT NOT NULL
                REFERENCES global_skill_roots(root_id) ON DELETE RESTRICT,
            role TEXT NOT NULL CHECK (role IN ('scan_only', 'activation_target')),
            PRIMARY KEY (agent_id, root_id)
         );
         CREATE UNIQUE INDEX one_activation_target_per_agent
            ON agent_global_roots(agent_id)
            WHERE role = 'activation_target';
         CREATE TABLE activations (
            skill_id TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
            target_root_id TEXT NOT NULL
                REFERENCES global_skill_roots(root_id) ON DELETE RESTRICT,
            directory_identity_key TEXT NOT NULL,
            desired_enabled INTEGER NOT NULL CHECK (desired_enabled IN (0, 1)),
            expected_entry_path TEXT NOT NULL,
            expected_target_path TEXT NOT NULL,
            observed_state TEXT NOT NULL CHECK (
                observed_state IN (
                    'present', 'missing', 'target_mismatch', 'dangling', 'occupied'
                )
            ),
            last_enabled_at TEXT,
            last_checked_at TEXT,
            PRIMARY KEY (skill_id, target_root_id)
         );
         CREATE UNIQUE INDEX active_activation_entry
            ON activations(target_root_id, directory_identity_key)
            WHERE desired_enabled = 1;
         CREATE TABLE recent_project_folders (
            canonical_path_key TEXT PRIMARY KEY,
            canonical_path TEXT NOT NULL,
            last_used_at TEXT NOT NULL
         );",
    )?;

    let legacy_agents = {
        let mut statement = transaction.prepare(
            "SELECT id, name, kind, skills_path, compatibility, created_at, updated_at
             FROM legacy_agents
             ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut names = BTreeMap::<String, String>::new();
    let mut roots = BTreeMap::<String, (String, String)>::new();
    let mut agents = Vec::with_capacity(legacy_agents.len());
    for (agent_id, raw_name, kind, skills_path, compatibility, created_at, updated_at) in
        legacy_agents
    {
        if agent_id.is_empty() {
            return Err(migration_constraint(
                "schema v8 migration found an empty Agent identity",
            ));
        }
        let name = raw_name.trim().to_owned();
        if !(1..=80).contains(&name.chars().count()) {
            return Err(migration_constraint(format!(
                "legacy Agent '{agent_id}' has an invalid name length"
            )));
        }
        let name_identity_key = agent_name_identity_key(&name);
        if let Some(existing) = names.insert(name_identity_key.clone(), agent_id.clone()) {
            return Err(migration_constraint(format!(
                "legacy Agents '{existing}' and '{agent_id}' have the same NFKC-casefold name"
            )));
        }
        let (configured_path, path_identity_key) =
            normalize_legacy_configured_path(&skills_path).map_err(migration_constraint)?;
        roots
            .entry(path_identity_key.clone())
            .or_insert_with(|| (configured_path.clone(), created_at.clone()));
        let (origin, preset_key) = match kind.as_str() {
            "claude_preset" => ("preset", Some("claude-code")),
            "codex_preset" => ("preset", Some("codex")),
            "custom" => ("custom", None),
            value => {
                return Err(migration_constraint(format!(
                    "legacy Agent '{agent_id}' has unknown kind '{value}'"
                )));
            }
        };
        if !matches!(compatibility.as_str(), "verified" | "unknown") {
            return Err(migration_constraint(format!(
                "legacy Agent '{agent_id}' has unknown compatibility '{compatibility}'"
            )));
        }
        agents.push(LegacyAgentMigrationRow {
            agent_id,
            origin,
            preset_key,
            name,
            name_identity_key,
            compatibility,
            path_identity_key,
            created_at,
            updated_at,
        });
    }

    let distinct_roots = roots
        .iter()
        .map(|(identity, (path, _))| (identity.clone(), PathBuf::from(path)))
        .collect::<Vec<_>>();
    for (index, (identity, path)) in distinct_roots.iter().enumerate() {
        for (other_identity, other_path) in distinct_roots.iter().skip(index + 1) {
            if path.starts_with(other_path) || other_path.starts_with(path) {
                return Err(migration_constraint(format!(
                    "legacy Target roots '{identity}' and '{other_identity}' overlap"
                )));
            }
        }
    }

    let mut root_ids = BTreeMap::<String, String>::new();
    for (identity, (configured_path, created_at)) in &roots {
        let root_id = new_remote_id(transaction)?;
        transaction.execute(
            "INSERT INTO global_skill_roots (
                root_id, configured_path, path_identity_key, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![root_id, configured_path, identity, created_at],
        )?;
        root_ids.insert(identity.clone(), root_id);
    }

    let mut agent_root_ids = BTreeMap::<String, String>::new();
    for agent in &agents {
        transaction.execute(
            "INSERT INTO agent_configurations (
                agent_id, origin, preset_key, name, name_identity_key,
                compatibility, project_skills_dir, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
            params![
                agent.agent_id,
                agent.origin,
                agent.preset_key,
                agent.name,
                agent.name_identity_key,
                agent.compatibility,
                agent.created_at,
                agent.updated_at
            ],
        )?;
        let root_id = root_ids
            .get(&agent.path_identity_key)
            .ok_or_else(|| migration_constraint("schema v8 migration lost a Target root"))?;
        transaction.execute(
            "INSERT INTO agent_global_roots (agent_id, root_id, role)
             VALUES (?1, ?2, 'activation_target')",
            params![agent.agent_id, root_id],
        )?;
        agent_root_ids.insert(agent.agent_id.clone(), root_id.clone());
    }

    let activation_identity_column =
        if table_has_column(transaction, "skills", "directory_identity_key")? {
            "directory_identity_key"
        } else {
            "identity_key"
        };
    let legacy_activations = {
        let mut statement = transaction.prepare(&format!(
            "SELECT
                legacy_activations.skill_id,
                legacy_activations.agent_id,
                legacy_activations.desired_enabled,
                legacy_activations.expected_entry_path,
                legacy_activations.expected_target_path,
                legacy_activations.observed_state,
                legacy_activations.last_enabled_at,
                legacy_activations.last_checked_at,
                skills.{activation_identity_column}
             FROM legacy_activations
             JOIN skills ON skills.id = legacy_activations.skill_id
             ORDER BY legacy_activations.skill_id, legacy_activations.agent_id",
        ))?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let root_paths_by_id = root_ids
        .iter()
        .map(|(identity, root_id)| {
            let path = &roots
                .get(identity)
                .expect("root ids are built from the same map")
                .0;
            (root_id.clone(), PathBuf::from(path))
        })
        .collect::<BTreeMap<_, _>>();
    let mut activations = BTreeMap::<(String, String), LegacyActivationAggregate>::new();
    for (
        skill_id,
        agent_id,
        desired_enabled,
        expected_entry_path,
        expected_target_path,
        observed_state,
        last_enabled_at,
        last_checked_at,
        directory_identity_key,
    ) in legacy_activations
    {
        if !matches!(
            observed_state.as_str(),
            "present" | "missing" | "target_mismatch" | "dangling" | "occupied"
        ) {
            return Err(migration_constraint(format!(
                "legacy Activation for Agent '{agent_id}' has unknown observed state \
                 '{observed_state}'"
            )));
        }
        let root_id = agent_root_ids.get(&agent_id).ok_or_else(|| {
            migration_constraint(format!(
                "legacy Activation references unknown Agent '{agent_id}'"
            ))
        })?;
        let root_path = root_paths_by_id
            .get(root_id)
            .ok_or_else(|| migration_constraint("schema v8 migration lost a Target path"))?;
        let normalized_entry = normalize_legacy_activation_entry(&expected_entry_path)
            .map_err(migration_constraint)?;
        let entry_path = PathBuf::from(&normalized_entry);
        if entry_path.parent() != Some(root_path.as_path()) {
            return Err(migration_constraint(format!(
                "legacy Activation entry '{expected_entry_path}' does not belong to \
                 Agent '{agent_id}' Target"
            )));
        }
        let (normalized_target, _) = normalize_legacy_configured_path(&expected_target_path)
            .map_err(migration_constraint)?;
        let aggregate = LegacyActivationAggregate {
            skill_id: skill_id.clone(),
            root_id: root_id.clone(),
            directory_identity_key,
            desired_enabled,
            expected_entry_path: normalized_entry,
            expected_target_path: normalized_target,
            observed_state,
            last_enabled_at,
            last_checked_at,
        };
        let key = (skill_id, root_id.clone());
        if let Some(existing) = activations.get_mut(&key) {
            let same_physical_fact = existing.skill_id == aggregate.skill_id
                && existing.root_id == aggregate.root_id
                && existing.directory_identity_key == aggregate.directory_identity_key
                && existing.desired_enabled == aggregate.desired_enabled
                && existing.expected_entry_path == aggregate.expected_entry_path
                && existing.expected_target_path == aggregate.expected_target_path
                && existing.observed_state == aggregate.observed_state;
            if !same_physical_fact {
                return Err(migration_constraint(format!(
                    "legacy Activations for Skill '{}' have ambiguous shared-Target state",
                    aggregate.skill_id
                )));
            }
            existing.last_enabled_at =
                latest_optional_timestamp(&existing.last_enabled_at, &aggregate.last_enabled_at);
            existing.last_checked_at =
                latest_optional_timestamp(&existing.last_checked_at, &aggregate.last_checked_at);
        } else {
            activations.insert(key, aggregate);
        }
    }

    for activation in activations.values() {
        transaction.execute(
            "INSERT INTO activations (
                skill_id, target_root_id, directory_identity_key, desired_enabled,
                expected_entry_path, expected_target_path, observed_state,
                last_enabled_at, last_checked_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                activation.skill_id,
                activation.root_id,
                activation.directory_identity_key,
                activation.desired_enabled,
                activation.expected_entry_path,
                activation.expected_target_path,
                activation.observed_state,
                activation.last_enabled_at,
                activation.last_checked_at
            ],
        )?;
    }

    let targetless_agents: i64 = transaction.query_row(
        "SELECT COUNT(*)
         FROM agent_configurations c
         WHERE NOT EXISTS (
            SELECT 1 FROM agent_global_roots r
            WHERE r.agent_id = c.agent_id AND r.role = 'activation_target'
         )",
        [],
        |row| row.get(0),
    )?;
    if targetless_agents != 0 {
        return Err(migration_constraint(format!(
            "schema v8 migration left {targetless_agents} Agent Configurations without a Target"
        )));
    }

    transaction.execute_batch("DROP TABLE legacy_activations; DROP TABLE legacy_agents;")?;
    let violations: i64 =
        transaction.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(migration_constraint(format!(
            "schema v8 migration left {violations} foreign-key violations"
        )));
    }
    let integrity: String =
        transaction.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("schema v8 migration failed the integrity check".into()),
        ));
    }
    transaction.execute(
        "UPDATE catalog_meta SET schema_version = 8 WHERE singleton = 1",
        [],
    )?;
    Ok(())
}

fn normalize_legacy_activation_entry(path: &str) -> Result<String, String> {
    let path = expand_legacy_home(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("legacy Activation entry '{path:?}' has no directory name"))?
        .to_owned();
    let parent = path
        .parent()
        .ok_or_else(|| format!("legacy Activation entry '{path:?}' has no Target parent"))?;
    let (parent, _) = normalize_legacy_configured_path(
        parent
            .to_str()
            .ok_or_else(|| "legacy Activation Target path is not UTF-8".to_owned())?,
    )?;
    let normalized = PathBuf::from(parent).join(name);
    normalized
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "legacy Activation entry path is not UTF-8".to_owned())
}

fn normalize_legacy_configured_path(path: &str) -> Result<(String, String), String> {
    let expanded = expand_legacy_home(path)?;
    if !expanded.is_absolute()
        || expanded.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(format!(
            "legacy configured path '{}' is not a safe absolute path",
            expanded.display()
        ));
    }
    let mut ancestor = expanded.clone();
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(&ancestor) {
            Ok(metadata) => {
                if !metadata.is_dir() && !metadata.file_type().is_symlink() {
                    return Err(format!(
                        "legacy configured path ancestor '{}' is not a directory",
                        ancestor.display()
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor.file_name().ok_or_else(|| {
                    format!(
                        "legacy configured path '{}' has no existing ancestor",
                        expanded.display()
                    )
                })?;
                missing.push(component.to_owned());
                if !ancestor.pop() {
                    return Err(format!(
                        "legacy configured path '{}' has no existing ancestor",
                        expanded.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "legacy configured path '{}' is unreadable: {error}",
                    ancestor.display()
                ));
            }
        }
    }
    let mut normalized = ancestor.canonicalize().map_err(|error| {
        format!(
            "legacy configured path ancestor '{}' cannot be canonicalized: {error}",
            ancestor.display()
        )
    })?;
    for component in missing.into_iter().rev() {
        normalized.push(component);
    }
    if normalized.parent().is_none() {
        return Err("the filesystem root cannot be an Agent Target".into());
    }
    let normalized = normalized
        .to_str()
        .ok_or_else(|| "legacy configured path is not UTF-8".to_owned())?
        .to_owned();
    let identity = configured_path_identity_key(&normalized);
    Ok((normalized, identity))
}

fn expand_legacy_home(path: &str) -> Result<PathBuf, String> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "HOME is unavailable while migrating a '~' Agent path".to_owned())?;
        let suffix = path.strip_prefix("~/").unwrap_or("");
        return Ok(PathBuf::from(home).join(suffix));
    }
    if path.starts_with('~') {
        return Err(format!(
            "legacy configured path '{path}' uses an unsupported home alias"
        ));
    }
    Ok(PathBuf::from(path))
}

fn latest_optional_timestamp(left: &Option<String>, right: &Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right).clone()),
        (Some(value), None) | (None, Some(value)) => Some(value.clone()),
        (None, None) => None,
    }
}

fn migration_constraint(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
        Some(message.into()),
    )
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
        Health::SourceSnapshotMismatch => "source_snapshot_mismatch",
    }
}

fn parse_health(value: &str) -> rusqlite::Result<Health> {
    match value {
        "healthy" => Ok(Health::Healthy),
        "broken" => Ok(Health::Broken),
        "modified" => Ok(Health::Modified),
        "source_snapshot_mismatch" => Ok(Health::SourceSnapshotMismatch),
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

/// `last_checked_at` historically holds either epoch seconds
/// (`unix_timestamp()`) or RFC3339 with optional millis
/// (`strftime('%Y-%m-%dT%H:%M:%fZ')`); normalize to epoch millis for
/// presentation; unknown shapes yield `None` = fail closed (never a guessed
/// timestamp).
fn parse_checked_at_ms(value: &str) -> Option<u64> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .map(|seconds| seconds.saturating_mul(1_000));
    }
    if !value.contains('.') {
        return crate::core::scan::rfc3339_to_epoch_millis(value);
    }
    // Split `SS[.fraction]Z` into the whole-second part and the millis
    // suffix, then parse the whole part with the project RFC3339 helper
    // (which already validates shape and range).
    let dot = value.find('.')?;
    let whole = value.get(0..dot)?;
    let tail = value.get(dot + 1..)?;
    let tail = tail.strip_suffix('Z').unwrap_or(tail);
    let mut fraction = tail
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    if fraction.is_empty() {
        return None;
    }
    while fraction.len() < 3 {
        fraction.push('0');
    }
    fraction.truncate(3);
    let fraction_ms = fraction.parse::<u64>().unwrap_or(0);
    let whole_ms = crate::core::scan::rfc3339_to_epoch_millis(whole)?;
    Some(whole_ms.saturating_add(fraction_ms))
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
    fn self_connection(path: &std::path::Path) -> Connection {
        Connection::open(path).expect("connection")
    }

    use super::*;
    use crate::core::home::BoundHome;
    use crate::seams::catalog_probe::CatalogProbe;

    fn bound_home() -> BoundHome {
        BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/skill-man-home"),
        )
    }

    fn catalog_artifacts(path: &std::path::Path) -> [Option<Vec<u8>>; 3] {
        [
            std::fs::read(path).ok(),
            std::fs::read(format!("{}-wal", path.display())).ok(),
            std::fs::read(format!("{}-shm", path.display())).ok(),
        ]
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
    fn git_source_transition_store_commits_and_undoes_one_v9_source() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let store = SqliteCatalogStore::open(&path).expect("fresh open");
        let record = crate::seams::source_transition_store::SourceTransitionRecord {
            remote_id: "remote-source-1".into(),
            provider: "github".into(),
            canonical_url: "https://github.com/acme/source".into(),
            aliases: Vec::new(),
            tracking_mode: "auto_release_tag_head".into(),
            tracking_value: None,
            selection_kind: "semver_tag".into(),
            selected_ref: "v1.0.0".into(),
            release_id: "release-1".into(),
            resolved_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            members: vec![
                crate::seams::source_transition_store::SourceTransitionMemberRecord {
                    skill_id: SkillId("source-skill-a".into()),
                    directory_name: "alpha".into(),
                    identity_key: "alpha".into(),
                    display_name: "Alpha".into(),
                    description: "First source member".into(),
                    storage_relpath: "skills/git/remote-source-1/source-skill-a".into(),
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
                    storage_relpath: "skills/git/remote-source-1/source-skill-b".into(),
                    skill_path: "skills/beta".into(),
                    tree_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                    provider_hash: Some("provider-hash".into()),
                },
            ],
        };
        SourceTransitionStore::validate_new_source_transition(&store, &record)
            .expect("one whole-source commit is valid");
        let snapshot = SourceTransitionStore::commit_source_transition(&store, record.clone())
            .expect("the v9 immutable transition commits");
        assert!(snapshot >= 1);
        assert!(
            SourceTransitionStore::source_transition_is_committed(&store, &record)
                .expect("whole release is current")
        );
        let connection = self_connection(&path);
        let sources: i64 = connection
            .query_row("SELECT COUNT(*) FROM git_repository_sources", [], |row| {
                row.get(0)
            })
            .expect("source count");
        let members: i64 = connection
            .query_row("SELECT COUNT(*) FROM git_source_members", [], |row| {
                row.get(0)
            })
            .expect("member count");
        assert_eq!((sources, members), (1, 2));

        let undo = SourceTransitionStore::undo_source_transition(&store, &record)
            .expect("whole-source Undo");
        assert!(undo >= 1);
        assert!(
            !SourceTransitionStore::source_transition_is_committed(&store, &record)
                .expect("undo removed the release")
        );
        drop(connection);
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
    fn recovery_open_preserves_catalog_and_wal_artifacts_byte_for_byte() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("skill-man.sqlite3");
        let bound = bound_home();
        SqliteCatalogStore::create_bound(&bound, &path).expect("create bound Catalog");
        let before = catalog_artifacts(&path);

        let reopened = SqliteCatalogStore::open_bound_without_catalog_mutation(&bound, &path)
            .expect("recovery-safe open");
        assert_eq!(reopened.startup_status().access, StartupAccess::ReadWrite);
        drop(reopened);

        assert_eq!(catalog_artifacts(&path), before);
    }

    #[test]
    fn recovery_open_refuses_old_schema_without_migrating_it() {
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
                        bound.bound_at,
                    ],
                )
                .expect("record identity");
        }
        let before = catalog_artifacts(&path);

        assert!(matches!(
            SqliteCatalogStore::open_bound_without_catalog_mutation(&bound, &path),
            Err(BoundCatalogOpenError::SchemaNotBound { found: 5 })
        ));
        assert_eq!(catalog_artifacts(&path), before);
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
