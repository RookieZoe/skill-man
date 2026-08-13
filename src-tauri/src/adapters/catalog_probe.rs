//! System `CatalogProbe` adapter: read-only identification of a Catalog
//! SQLite file. Opens with SQLITE_OPEN_READ_ONLY, never migrates, seeds or
//! writes; integrity and foreign-key checks plus the Home identity columns
//! come straight from the file.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::core::home::HomeId;
use crate::seams::catalog_probe::{
    CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
};

pub struct SqliteCatalogProbe;

impl SqliteCatalogProbe {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqliteCatalogProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogProbe for SqliteCatalogProbe {
    fn probe(&self, path: &Path) -> Result<CatalogProbeReport, CatalogProbeError> {
        if !path.exists() {
            return Ok(CatalogProbeReport::absent());
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| CatalogProbeError::Unreadable(format!("{}: {error}", path.display())))?;

        // A file that exists but is not a readable Catalog identifies as
        // exists-without-schema; bootstrap treats that as a closed mismatch.
        let schema_version: Option<u32> = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;
        let Some(schema_version) = schema_version else {
            return Ok(CatalogProbeReport {
                exists: true,
                schema_version: None,
                integrity_ok: false,
                foreign_keys_ok: false,
                home_identity: None,
                snapshot_version: None,
            });
        };

        let integrity_ok = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .ok()
            .as_deref()
            == Some("ok");
        let foreign_keys_ok = connection
            .prepare("PRAGMA foreign_key_check")
            .map(|mut statement| {
                statement
                    .query_map([], |_| Ok(()))
                    .map(|rows| rows.count() == 0)
                    .unwrap_or(false)
            })
            .unwrap_or(false);

        let home_identity = if schema_version >= 5 {
            read_home_identity(&connection)
        } else {
            None
        };
        let snapshot_version: Option<i64> = connection
            .query_row(
                "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let snapshot_version = snapshot_version.and_then(|value| u64::try_from(value).ok());

        Ok(CatalogProbeReport {
            exists: true,
            schema_version: Some(schema_version),
            integrity_ok,
            foreign_keys_ok,
            home_identity,
            snapshot_version,
        })
    }
}

fn read_home_identity(connection: &Connection) -> Option<CatalogHomeIdentity> {
    let (home_id, volume_fsid, volume_uuid, home_bound_at): (String, String, String, String) =
        connection
            .query_row(
                "SELECT home_id, volume_fsid, volume_uuid, home_bound_at
                 FROM catalog_meta WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    ))
                },
            )
            .optional()
            .ok()
            .flatten()?;
    let home_id = HomeId::parse(&home_id)?;
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
