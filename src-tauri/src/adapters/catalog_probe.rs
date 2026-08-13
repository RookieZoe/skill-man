//! System `CatalogProbe` adapter: read-only identification of a Catalog
//! SQLite file. Opens with SQLITE_OPEN_READ_ONLY, never migrates, seeds or
//! writes; integrity and foreign-key checks plus the Home identity columns
//! come straight from the file.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::core::home::HomeId;
use crate::seams::catalog_probe::{
    CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
    FixtureAgentRowEvidence, FixtureCatalogEvidence, FixtureCatalogMetaEvidence,
    FixturePreferencesEvidence, FixtureSkillRowEvidence,
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

    fn probe_fixture(&self, path: &Path) -> Result<FixtureCatalogEvidence, CatalogProbeError> {
        // A missing Catalog is the fact "no Catalog": the classifier pairs it
        // with the tree facts. Anything that exists but cannot be read fully
        // is a hard error — the Home classifies unknown, never partial.
        if !path.exists() {
            return Ok(FixtureCatalogEvidence::clean());
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| CatalogProbeError::Unreadable(format!("{}: {error}", path.display())))?;

        let mut tables: Vec<String> = connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .query_map([], |row| row.get(0))
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;
        tables.sort();

        let meta = connection
            .query_row(
                "SELECT schema_version, first_run_completed_at
                 FROM catalog_meta WHERE singleton = 1",
                [],
                |row| {
                    Ok(FixtureCatalogMetaEvidence {
                        schema_version: row.get(0)?,
                        first_run_completed_at: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;

        let skills = connection
            .prepare(
                "SELECT id, directory_name, identity_key, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                 FROM skills ORDER BY id",
            )
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .query_map([], |row| {
                Ok(FixtureSkillRowEvidence {
                    id: row.get(0)?,
                    directory_name: row.get(1)?,
                    identity_key: row.get(2)?,
                    display_name: row.get(3)?,
                    description: row.get(4)?,
                    source_kind: row.get(5)?,
                    library_entry_path: row.get(6)?,
                    final_entity_path: row.get(7)?,
                    recorded_content_hash: row.get(8)?,
                    health: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;

        let agents = connection
            .prepare(
                "SELECT id, name, kind, skills_path, path_identity_key, detected,
                        compatibility, created_at, updated_at
                 FROM agents ORDER BY id",
            )
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .query_map([], |row| {
                Ok(FixtureAgentRowEvidence {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    kind: row.get(2)?,
                    skills_path: row.get(3)?,
                    path_identity_key: row.get(4)?,
                    detected: row.get::<_, i64>(5)? != 0,
                    compatibility: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;

        let preferences = connection
            .query_row(
                "SELECT launch_at_login, show_in_dock, check_app_updates,
                        check_skill_updates, last_app_update_check_at,
                        last_skill_update_check_at
                 FROM preferences WHERE singleton = 1",
                [],
                |row| {
                    Ok(FixturePreferencesEvidence {
                        launch_at_login: row.get::<_, i64>(0)? != 0,
                        show_in_dock: row.get::<_, i64>(1)? != 0,
                        check_app_updates: row.get::<_, i64>(2)? != 0,
                        check_skill_updates: row.get::<_, i64>(3)? != 0,
                        last_app_update_check_at: row.get(4)?,
                        last_skill_update_check_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;

        let count = |table: &str| -> Result<u64, CatalogProbeError> {
            let statement = format!("SELECT COUNT(*) FROM {table}");
            connection
                .query_row(&statement, [], |row| row.get::<_, i64>(0))
                .map(|value| u64::try_from(value).unwrap_or(u64::MAX))
                .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))
        };

        Ok(FixtureCatalogEvidence {
            tables,
            meta,
            skills,
            agents,
            preferences,
            activation_count: count("activations")?,
            file_source_count: count("file_sources")?,
            remote_source_count: count("remote_sources")?,
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
