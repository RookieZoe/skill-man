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

#[derive(Debug, Error)]
pub enum CatalogProbeError {
    #[error("the catalog could not be opened read-only: {0}")]
    Unreadable(String),
}

/// Seam: read-only identification of a Catalog SQLite file. The system
/// adapter opens with SQLITE_OPEN_READ_ONLY and never migrates or seeds.
pub trait CatalogProbe: Send + Sync {
    fn probe(&self, path: &Path) -> Result<CatalogProbeReport, CatalogProbeError>;
}
