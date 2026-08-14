//! Legacy Catalog migration seam (§3.4, §5.4): the one-time Home
//! Binding/Legacy transition migrates a pre-identity Catalog (v1–v4) to the
//! current schema with the new Home identity recorded and verified. The
//! system adapter backs the untouched Catalog up first and checkpoints the
//! WAL so the copy transition carries a consistent SQLite+WAL+SHM set.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::seams::catalog_probe::CatalogHomeIdentity;

#[derive(Debug, Error)]
pub enum LegacyMigrationError {
    #[error("the Catalog is not a pre-identity schema: {found}")]
    NotPreIdentity { found: u32 },
    #[error("could not back up the Catalog before migration: {0}")]
    Backup(String),
    #[error("could not open or configure the Catalog: {0}")]
    Open(String),
    #[error("could not checkpoint the Catalog WAL: {0}")]
    Checkpoint(String),
    #[error("could not migrate the Catalog: {0}")]
    Migration(String),
    #[error("could not write or verify the Home identity: {0}")]
    Identity(String),
}

/// Seam: quiesce and migrate a pre-identity Catalog. Only the Home
/// Binding/Legacy flow calls this; bootstrap probes read-only (spec §3.4).
pub trait LegacyCatalogMigrator: Send + Sync {
    /// Checkpoint the Catalog WAL into the main database (TRUNCATE) so a
    /// later tree copy carries the consistent SQLite+WAL+SHM set. The caller
    /// must have proven no other process holds the WAL index.
    fn checkpoint(&self, path: &Path) -> Result<(), LegacyMigrationError>;

    /// Migrate a pre-identity Catalog to the current schema and record the
    /// Home identity; returns the backup path. Backs up the untouched file
    /// first, checkpoints the WAL, then migrates and verifies the identity.
    fn migrate_with_identity(
        &self,
        path: &Path,
        identity: &CatalogHomeIdentity,
    ) -> Result<PathBuf, LegacyMigrationError>;
}
