//! Prepared Home Catalog creation seam (§4.4, §5.2): Fixture Recovery builds
//! a clean Home offline and must create its Catalog without a verified
//! `BoundHome` value object — Legacy mode leaves identity null, Bound Restore
//! mode records the same `home_id` under validation. The system adapter
//! never opens or overwrites an existing file.

use std::path::Path;

use thiserror::Error;

use crate::seams::catalog_probe::CatalogHomeIdentity;

#[derive(Debug, Error)]
pub enum PreparedCatalogError {
    #[error("a Catalog already exists at {0}")]
    AlreadyExists(String),
    #[error("could not create the prepared Catalog: {0}")]
    Create(String),
    #[error("could not record the Home identity in the prepared Catalog: {0}")]
    Identity(String),
    #[error("prepared Catalog verification failed: {0}")]
    Verify(String),
}

/// Seam: create a fresh v5 Catalog at `path` for a prepared Home. With
/// `identity` (Bound Restore) the Catalog carries the same Home identity the
/// binding proves; without it (Legacy) identity columns stay null until the
/// Home Binding flow (#45) commits them.
pub trait PreparedCatalogFactory: Send + Sync {
    fn create_prepared(
        &self,
        path: &Path,
        identity: Option<&CatalogHomeIdentity>,
    ) -> Result<(), PreparedCatalogError>;
}
