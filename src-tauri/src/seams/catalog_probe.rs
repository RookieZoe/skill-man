//! Read-only Catalog probe seam (§4.2): identifies schema, integrity,
//! foreign keys and Home identity of a Catalog SQLite file without ever
//! migrating, seeding or writing. The bootstrap authority decides writable
//! open only after the probe and the locator/marker/volume checks agree.

use std::path::Path;

use thiserror::Error;

use crate::core::home::HomeId;

/// The newest Catalog schema this build supports. Bumps only via the schema
/// migration tickets (v5: Home identity — ticket #41; v6: Remote Source
/// Parent — ticket #48).
pub const CURRENT_CATALOG_SCHEMA_VERSION: u32 = 5;

/// The Home identity recorded in `catalog_meta` (schema v5+).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogHomeIdentity {
    pub home_id: HomeId,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub home_bound_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogProbeReport {
    /// The SQLite file exists at the expected path.
    pub exists: bool,
    /// `None` when the file exists but its schema cannot be identified.
    pub schema_version: Option<u32>,
    /// `PRAGMA integrity_check` passed.
    pub integrity_ok: bool,
    /// `PRAGMA foreign_key_check` found no violations.
    pub foreign_keys_ok: bool,
    /// Home identity columns, when readable and valid.
    pub home_identity: Option<CatalogHomeIdentity>,
    /// `catalog_meta.snapshot_version`, when readable.
    pub snapshot_version: Option<u64>,
}

impl CatalogProbeReport {
    pub fn absent() -> Self {
        Self {
            exists: false,
            schema_version: None,
            integrity_ok: false,
            foreign_keys_ok: true,
            home_identity: None,
            snapshot_version: None,
        }
    }
}

/// Raw `catalog_meta` columns of a fixture-shaped Catalog (spec §3.5). Only
/// `schema_version` and `first_run_completed_at` participate in the exact
/// fingerprint: `snapshot_version` and `last_startup_check_at` are runtime
/// maintained meta fields the old app rewrote on every launch, so they can
/// never be evidence (spec: a single meta value is not sufficient evidence).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureCatalogMetaEvidence {
    pub schema_version: u32,
    pub first_run_completed_at: Option<String>,
}

/// Raw `skills` row columns, exactly as stored (spec §3.5: the classifier
/// matches byte-exact tuples, never names or UI content alone).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureSkillRowEvidence {
    pub id: String,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub source_kind: String,
    pub library_entry_path: Option<String>,
    pub final_entity_path: String,
    pub recorded_content_hash: Option<String>,
    pub health: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Raw `agents` row columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureAgentRowEvidence {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub skills_path: String,
    pub path_identity_key: String,
    pub detected: bool,
    pub compatibility: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Raw `preferences` row columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixturePreferencesEvidence {
    pub launch_at_login: bool,
    pub show_in_dock: bool,
    pub check_app_updates: bool,
    pub check_skill_updates: bool,
    pub last_app_update_check_at: Option<String>,
    pub last_skill_update_check_at: Option<String>,
}

/// Read-only fixture evidence of a Catalog file (spec §3.5): the exact table
/// list and raw row shapes the fixture classifier matches against the
/// immutable `FixtureFingerprintV1` constants. The adapter never interprets
/// values; a read failure anywhere is an `Unknown` classification, never a
/// partial match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureCatalogEvidence {
    /// User table names in `sqlite_master`, sorted.
    pub tables: Vec<String>,
    pub meta: Option<FixtureCatalogMetaEvidence>,
    pub skills: Vec<FixtureSkillRowEvidence>,
    pub agents: Vec<FixtureAgentRowEvidence>,
    pub preferences: Option<FixturePreferencesEvidence>,
    pub activation_count: u64,
    pub file_source_count: u64,
    pub remote_source_count: u64,
}

impl FixtureCatalogEvidence {
    /// A readable Catalog with no rows at all: the shape of a fresh Home.
    pub fn clean() -> Self {
        Self {
            tables: Vec::new(),
            meta: None,
            skills: Vec::new(),
            agents: Vec::new(),
            preferences: None,
            activation_count: 0,
            file_source_count: 0,
            remote_source_count: 0,
        }
    }
}

#[derive(Debug, Error)]
pub enum CatalogProbeError {
    #[error("the catalog could not be opened read-only: {0}")]
    Unreadable(String),
    /// The file opened as SQLite but is not a readable Catalog (e.g.
    /// SQLITE_NOTADB or a missing catalog schema): a content inconsistency
    /// of the site, not a reachability failure.
    #[error("the catalog file is not a valid Catalog: {0}")]
    Invalid(String),
}

/// Seam: read-only identification of a Catalog SQLite file. The system
/// adapter opens with SQLITE_OPEN_READ_ONLY and never migrates or seeds.
pub trait CatalogProbe: Send + Sync {
    fn probe(&self, path: &Path) -> Result<CatalogProbeReport, CatalogProbeError>;

    /// Read-only fixture evidence (spec §3.5). The default fails closed so a
    /// probe that cannot produce evidence classifies the Home as unknown and
    /// keeps the recovery lock — never a guess at "clean".
    fn probe_fixture(&self, path: &Path) -> Result<FixtureCatalogEvidence, CatalogProbeError> {
        let _ = path;
        Err(CatalogProbeError::Unreadable(
            "fixture evidence probe is not implemented by this adapter".into(),
        ))
    }
}
