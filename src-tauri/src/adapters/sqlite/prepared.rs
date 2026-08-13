//! System `PreparedCatalogFactory` adapter (spec §5.2): creates a fresh v5
//! Catalog for a prepared recovery Home — Legacy mode with identity columns
//! null, Bound Restore mode with the binding's identity. Refuses an existing
//! file so it can never overwrite real data.

use std::path::Path;

use rusqlite::{Connection, TransactionBehavior, params};

use crate::seams::catalog_probe::CatalogHomeIdentity;
use crate::seams::prepared_catalog::{PreparedCatalogError, PreparedCatalogFactory};

use super::{configure_connection, has_content, migrate_to_current, read_catalog_identity};

pub struct SqlitePreparedCatalogFactory;

impl PreparedCatalogFactory for SqlitePreparedCatalogFactory {
    fn create_prepared(
        &self,
        path: &Path,
        identity: Option<&CatalogHomeIdentity>,
    ) -> Result<(), PreparedCatalogError> {
        if has_content(path) {
            return Err(PreparedCatalogError::AlreadyExists(
                path.display().to_string(),
            ));
        }
        let mut connection = Connection::open(path)
            .map_err(|error| PreparedCatalogError::Create(error.to_string()))?;
        configure_connection(&connection)
            .map_err(|error| PreparedCatalogError::Create(error.to_string()))?;
        migrate_to_current(&mut connection, 0)
            .map_err(|error| PreparedCatalogError::Create(error.to_string()))?;
        if let Some(identity) = identity {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| PreparedCatalogError::Identity(error.to_string()))?;
            transaction
                .execute(
                    "UPDATE catalog_meta
                     SET home_id = ?1, volume_fsid = ?2, volume_uuid = ?3, home_bound_at = ?4
                     WHERE singleton = 1",
                    params![
                        identity.home_id.0,
                        identity.volume_fsid,
                        identity.volume_uuid,
                        identity.home_bound_at,
                    ],
                )
                .map_err(|error| PreparedCatalogError::Identity(error.to_string()))?;
            transaction
                .commit()
                .map_err(|error| PreparedCatalogError::Identity(error.to_string()))?;
            let stored = read_catalog_identity(&connection)
                .ok_or_else(|| PreparedCatalogError::Verify("identity unreadable".into()))?;
            if stored.home_id != identity.home_id
                || stored.volume_fsid != identity.volume_fsid
                || stored.volume_uuid != identity.volume_uuid
            {
                return Err(PreparedCatalogError::Verify(
                    "written identity does not match the binding".into(),
                ));
            }
        } else {
            let stored = read_catalog_identity(&connection);
            if stored.is_some() {
                return Err(PreparedCatalogError::Verify(
                    "Legacy prepared Catalog unexpectedly carries identity".into(),
                ));
            }
        }
        Ok(())
    }
}
