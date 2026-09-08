//! System `CatalogProbe` adapter: read-only identification of a Catalog
//! SQLite file. Opens with SQLITE_OPEN_READ_ONLY, never migrates, seeds or
//! writes; integrity and foreign-key checks plus the Home identity columns
//! come straight from the file.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::core::home::HomeId;
use crate::seams::catalog_probe::{
    CatalogHomeIdentity, CatalogProbe, CatalogProbeError, CatalogProbeReport,
    CatalogRecoveryIdentity, CatalogRecoveryProfile, FixtureAgentRowEvidence,
    FixtureCatalogEvidence, FixtureCatalogMetaEvidence, FixturePreferencesEvidence,
    FixtureSkillRowEvidence,
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
        .map_err(|error| map_open_error(error, path))?;

        // A file that exists but is not a readable Catalog identifies as
        // exists-without-schema; bootstrap treats that as a closed mismatch.
        let schema_version: Option<u32> = connection
            .query_row(
                "SELECT schema_version FROM catalog_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_query_error)?;
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

    fn probe_recovery_profile(
        &self,
        path: &Path,
    ) -> Result<CatalogRecoveryProfile, CatalogProbeError> {
        if !path.exists() {
            return Ok(CatalogRecoveryProfile::absent());
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| map_open_error(error, path))?;

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
        let identity = connection
            .query_row(
                "SELECT home_id, home_bound_at FROM catalog_meta WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .ok()
            .flatten()
            .and_then(|(home_id, created_at)| {
                Some(CatalogRecoveryIdentity {
                    home_id: HomeId::parse(&home_id)?,
                    created_at: (!created_at.is_empty()).then_some(created_at)?,
                })
            });

        Ok(CatalogRecoveryProfile {
            exists: true,
            integrity_ok,
            foreign_keys_ok,
            identity,
            required_capabilities: has_recovery_capabilities(&connection),
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

        // The fixture classifier compares both the Legacy v4 shape
        // (`identity_key`) and the current v9 shape
        // (`directory_identity_key`). The column name is picked from the
        // actual schema; the evidence keeps the legacy field name.
        let skills_columns = connection
            .prepare("SELECT name FROM pragma_table_info('skills')")
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?;
        let identity_column = if skills_columns.contains("directory_identity_key") {
            "directory_identity_key"
        } else {
            "identity_key"
        };
        let skills = connection
            .prepare(&format!(
                "SELECT id, directory_name, {identity_column}, display_name, description,
                        source_kind, library_entry_path, final_entity_path,
                        recorded_content_hash, health, created_at, updated_at
                 FROM skills ORDER BY id",
            ))
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

        // Fixture agent evidence is shape-dependent: the Legacy footprint
        // carries the historical `agents` tuples, while a current-schema
        // (v8) footprint carries the same facts as one Agent Configuration
        // plus its unique Activation Target root (spec §3.4 v8).
        let agents = if tables.iter().any(|table| table == "agents") {
            connection
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
                .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
        } else if tables.iter().any(|table| table == "agent_configurations") {
            connection
                .prepare(
                    "SELECT
                        configurations.agent_id,
                        configurations.name,
                        configurations.origin,
                        configurations.preset_key,
                        configurations.compatibility,
                        configurations.created_at,
                        configurations.updated_at,
                        roots.configured_path,
                        roots.path_identity_key
                     FROM agent_configurations configurations
                     JOIN agent_global_roots memberships
                       ON memberships.agent_id = configurations.agent_id
                      AND memberships.role = 'activation_target'
                     JOIN global_skill_roots roots
                       ON roots.root_id = memberships.root_id
                     ORDER BY configurations.agent_id",
                )
                .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
                .query_map([], |row| {
                    let origin: String = row.get(2)?;
                    let preset_key: Option<String> = row.get(3)?;
                    Ok(FixtureAgentRowEvidence {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        kind: match (origin.as_str(), preset_key.as_deref()) {
                            ("preset", Some("claude-code")) => "claude_preset".to_owned(),
                            ("preset", Some("codex")) => "codex_preset".to_owned(),
                            _ => "custom".to_owned(),
                        },
                        skills_path: row.get(7)?,
                        path_identity_key: row.get(8)?,
                        detected: true,
                        compatibility: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })
                .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| CatalogProbeError::Unreadable(error.to_string()))?
        } else {
            Vec::new()
        };

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

        let known_tables = tables.clone();
        let count = |table: &str| -> Result<u64, CatalogProbeError> {
            // Current-schema Catalogs (v6+) no longer carry the legacy
            // `remote_sources` table; a missing table counts as zero rows
            // instead of failing the whole probe (the fixture footprint
            // only exists in the v4 shape).
            if !known_tables.iter().any(|known| known == table) {
                return Ok(0);
            }
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

fn has_recovery_capabilities(connection: &Connection) -> bool {
    // These are actual runtime capabilities, not the Catalog's declared
    // schema_version. Extra/future tables are accepted when this complete
    // product surface remains present. Keep the required layout explicit:
    // `foreign_key_check` proves existing rows, but an empty, partially
    // recreated Catalog can pass it after an FK, PK or UNIQUE constraint was
    // removed.
    const TABLES: &[(&str, &[&str])] = &[
        (
            "catalog_meta",
            &[
                "singleton",
                "schema_version",
                "snapshot_version",
                "first_run_completed_at",
                "last_startup_check_at",
                "home_id",
                "volume_fsid",
                "volume_uuid",
                "home_bound_at",
            ],
        ),
        (
            "skills",
            &[
                "id",
                "directory_name",
                "directory_identity_key",
                "display_name",
                "description",
                "source_kind",
                "library_entry_path",
                "final_entity_path",
                "recorded_content_hash",
                "health",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "agent_configurations",
            &[
                "agent_id",
                "origin",
                "preset_key",
                "name",
                "name_identity_key",
                "compatibility",
                "project_skills_dir",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "global_skill_roots",
            &[
                "root_id",
                "configured_path",
                "path_identity_key",
                "created_at",
                "updated_at",
            ],
        ),
        ("agent_global_roots", &["agent_id", "root_id", "role"]),
        (
            "activations",
            &[
                "skill_id",
                "target_root_id",
                "directory_identity_key",
                "desired_enabled",
                "expected_entry_path",
                "expected_target_path",
                "observed_state",
                "last_enabled_at",
                "last_checked_at",
            ],
        ),
        (
            "recent_project_folders",
            &["canonical_path_key", "canonical_path", "last_used_at"],
        ),
        (
            "file_sources",
            &[
                "skill_id",
                "original_path",
                "original_filename",
                "installed_at",
            ],
        ),
        (
            "remote_source_parents",
            &["remote_id", "canonical_url", "created_at"],
        ),
        (
            "remote_source_aliases",
            &["remote_id", "alias_url", "confirmed_at"],
        ),
        (
            "remote_bindings",
            &[
                "skill_id",
                "remote_id",
                "requested_ref",
                "verification_anchor_commit",
                "original_commit_known",
                "skill_path",
                "provider_hash",
                "remote_baseline_hash",
                "current_baseline_hash",
                "last_checked_at",
                "last_updated_at",
            ],
        ),
        (
            "git_repository_sources",
            &[
                "remote_id",
                "provider",
                "canonical_url",
                "tracking_mode",
                "tracking_value",
                "current_selected_ref",
                "current_release_id",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "git_source_releases",
            &[
                "release_id",
                "remote_id",
                "selection_kind",
                "selected_ref",
                "resolved_commit",
                "discovered_at",
            ],
        ),
        (
            "git_source_release_members",
            &[
                "release_id",
                "skill_id",
                "skill_path",
                "directory_name",
                "directory_identity_key",
                "tree_hash",
                "provider_hash",
            ],
        ),
        (
            "git_source_members",
            &[
                "skill_id",
                "remote_id",
                "skill_path",
                "storage_relpath",
                "presence",
                "first_seen_release_id",
                "last_seen_release_id",
                "last_checked_at",
                "last_updated_at",
            ],
        ),
        (
            "preferences",
            &[
                "singleton",
                "launch_at_login",
                "show_in_dock",
                "check_app_updates",
                "check_skill_updates",
                "last_app_update_check_at",
                "last_skill_update_check_at",
            ],
        ),
    ];
    const PRIMARY_KEYS: &[(&str, &[&str])] = &[
        ("catalog_meta", &["singleton"]),
        ("skills", &["id"]),
        ("agent_configurations", &["agent_id"]),
        ("global_skill_roots", &["root_id"]),
        ("agent_global_roots", &["agent_id", "root_id"]),
        ("activations", &["skill_id", "target_root_id"]),
        ("recent_project_folders", &["canonical_path_key"]),
        ("file_sources", &["skill_id"]),
        ("remote_source_parents", &["remote_id"]),
        ("remote_source_aliases", &["remote_id", "alias_url"]),
        ("remote_bindings", &["skill_id"]),
        ("git_repository_sources", &["remote_id"]),
        ("git_source_releases", &["release_id"]),
        ("git_source_release_members", &["release_id", "skill_path"]),
        ("git_source_members", &["skill_id"]),
        ("preferences", &["singleton"]),
    ];
    const UNIQUE_INDEXES: &[(&str, &[&str])] = &[
        ("agent_configurations", &["name_identity_key"]),
        ("global_skill_roots", &["path_identity_key"]),
        ("agent_global_roots", &["agent_id"]),
        ("activations", &["target_root_id", "directory_identity_key"]),
        ("remote_source_parents", &["canonical_url"]),
        ("remote_source_aliases", &["alias_url"]),
        ("git_repository_sources", &["provider", "canonical_url"]),
        (
            "git_source_releases",
            &["remote_id", "selected_ref", "resolved_commit"],
        ),
        ("git_source_release_members", &["release_id", "skill_id"]),
        ("git_source_members", &["remote_id", "skill_path"]),
        ("git_source_members", &["remote_id", "storage_relpath"]),
    ];
    const FOREIGN_KEYS: &[(&str, &str, &str, &str, &str)] = &[
        ("activations", "skill_id", "skills", "id", "CASCADE"),
        (
            "activations",
            "target_root_id",
            "global_skill_roots",
            "root_id",
            "RESTRICT",
        ),
        (
            "agent_global_roots",
            "agent_id",
            "agent_configurations",
            "agent_id",
            "CASCADE",
        ),
        (
            "agent_global_roots",
            "root_id",
            "global_skill_roots",
            "root_id",
            "RESTRICT",
        ),
        ("file_sources", "skill_id", "skills", "id", "CASCADE"),
        (
            "remote_source_aliases",
            "remote_id",
            "remote_source_parents",
            "remote_id",
            "CASCADE",
        ),
        ("remote_bindings", "skill_id", "skills", "id", "CASCADE"),
        (
            "remote_bindings",
            "remote_id",
            "remote_source_parents",
            "remote_id",
            "CASCADE",
        ),
        (
            "git_repository_sources",
            "remote_id",
            "remote_source_parents",
            "remote_id",
            "CASCADE",
        ),
        (
            "git_repository_sources",
            "current_release_id",
            "git_source_releases",
            "release_id",
            "NO ACTION",
        ),
        (
            "git_source_releases",
            "remote_id",
            "remote_source_parents",
            "remote_id",
            "CASCADE",
        ),
        (
            "git_source_release_members",
            "release_id",
            "git_source_releases",
            "release_id",
            "CASCADE",
        ),
        (
            "git_source_release_members",
            "skill_id",
            "skills",
            "id",
            "NO ACTION",
        ),
        ("git_source_members", "skill_id", "skills", "id", "CASCADE"),
        (
            "git_source_members",
            "remote_id",
            "remote_source_parents",
            "remote_id",
            "CASCADE",
        ),
        (
            "git_source_members",
            "first_seen_release_id",
            "git_source_releases",
            "release_id",
            "NO ACTION",
        ),
        (
            "git_source_members",
            "last_seen_release_id",
            "git_source_releases",
            "release_id",
            "NO ACTION",
        ),
    ];

    TABLES.iter().all(|(table, columns)| {
        let Ok(mut statement) = connection.prepare("SELECT name FROM pragma_table_info(?1)") else {
            return false;
        };
        let Ok(rows) = statement.query_map([*table], |row| row.get::<_, String>(0)) else {
            return false;
        };
        let names = rows.collect::<Result<std::collections::BTreeSet<_>, _>>();
        let Ok(names) = names else {
            return false;
        };
        columns.iter().all(|column| names.contains(*column))
    }) && PRIMARY_KEYS
        .iter()
        .all(|(table, columns)| has_primary_key(connection, table, columns))
        && UNIQUE_INDEXES
            .iter()
            .all(|(table, columns)| has_unique_index(connection, table, columns))
        && FOREIGN_KEYS
            .iter()
            .all(|(table, from, referenced_table, to, on_delete)| {
                has_foreign_key(connection, table, from, referenced_table, to, on_delete)
            })
        && agent_configuration_rows_are_complete(connection)
}

fn agent_configuration_rows_are_complete(connection: &Connection) -> bool {
    let invalid_target_memberships = connection.query_row(
        "SELECT COUNT(*)
         FROM agent_configurations configuration
         WHERE (
            SELECT COUNT(*)
            FROM agent_global_roots membership
            WHERE membership.agent_id = configuration.agent_id
              AND membership.role = 'activation_target'
         ) != 1",
        [],
        |row| row.get::<_, i64>(0),
    );
    // Deleting the last Agent Configuration retains disabled Activation
    // history and its Root. Only enabled Activations still require a consumer,
    // matching the configuration store's guard_removed_target contract.
    let orphan_activation_targets = connection.query_row(
        "SELECT COUNT(*)
         FROM activations activation
         WHERE activation.desired_enabled = 1
           AND NOT EXISTS (
            SELECT 1
            FROM agent_global_roots membership
            WHERE membership.root_id = activation.target_root_id
              AND membership.role = 'activation_target'
         )",
        [],
        |row| row.get::<_, i64>(0),
    );
    matches!(invalid_target_memberships, Ok(0)) && matches!(orphan_activation_targets, Ok(0))
}

fn has_primary_key(connection: &Connection, table: &str, expected: &[&str]) -> bool {
    let keys = connection
        .prepare("SELECT name, pk FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")
        .and_then(|mut statement| {
            statement
                .query_map([table], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        });
    keys.map(|keys| keys.iter().map(String::as_str).eq(expected.iter().copied()))
        .unwrap_or(false)
}

fn has_unique_index(connection: &Connection, table: &str, expected: &[&str]) -> bool {
    let Ok(mut indexes) =
        connection.prepare("SELECT name FROM pragma_index_list(?1) WHERE \"unique\" = 1")
    else {
        return false;
    };
    let Ok(indexes) = indexes
        .query_map([table], |row| row.get::<_, String>(0))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    else {
        return false;
    };
    indexes.into_iter().any(|index| {
        connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .and_then(|mut statement| {
                statement
                    .query_map([index], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map(|columns| {
                columns
                    .iter()
                    .map(String::as_str)
                    .eq(expected.iter().copied())
            })
            .unwrap_or(false)
    })
}

fn has_foreign_key(
    connection: &Connection,
    table: &str,
    from: &str,
    referenced_table: &str,
    to: &str,
    on_delete: &str,
) -> bool {
    connection
        .prepare(
            "SELECT \"table\", \"from\", \"to\", on_delete
             FROM pragma_foreign_key_list(?1)",
        )
        .and_then(|mut statement| {
            statement
                .query_map([table], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map(|references| {
            references
                .iter()
                .any(|(actual_table, actual_from, actual_to, actual_on_delete)| {
                    actual_table == referenced_table
                        && actual_from == from
                        && actual_to == to
                        && actual_on_delete == on_delete
                })
        })
        .unwrap_or(false)
}

/// The open step fails for two distinct reasons: a file that is not a
/// database (SQLITE_NOTADB — a content fact) and everything else
/// (permission, lock, transient I/O — a reachability fact). The bootstrap
/// authority routes them to HomeIdentityMismatch and HomeUnavailable
/// respectively (spec §5.5).
fn map_open_error(error: rusqlite::Error, path: &Path) -> CatalogProbeError {
    use rusqlite::Error as SqliteError;
    match &error {
        SqliteError::SqliteFailure(ffi, _)
            if ffi.code == rusqlite::ffi::ErrorCode::NotADatabase =>
        {
            CatalogProbeError::Invalid(format!("{}: {error}", path.display()))
        }
        _ => CatalogProbeError::Unreadable(format!("{}: {error}", path.display())),
    }
}

/// After a successful open, every query failure is a content fact: the
/// file is a readable SQLite database that is not (or no longer) a valid
/// Catalog.
fn map_query_error(error: rusqlite::Error) -> CatalogProbeError {
    CatalogProbeError::Invalid(error.to_string())
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
