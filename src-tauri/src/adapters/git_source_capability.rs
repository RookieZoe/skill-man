//! SQLite + Home manifest adapter for the read-only Source Capability Scan.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::core::write_gate::WriteGate;
use crate::seams::filesystem::FileSystem;
use crate::seams::git_source_capability::{
    GitRepositorySourceFact, GitSourceCapabilityFacts, GitSourceCapabilityReader,
    GitSourceCatalogStructure, GitSourceFact, GitSourceManifestFact, GitSourceMemberFact,
    GitSourceReleaseFact,
};

/// The system adapter opens the active Bound Home's Catalog read-only on
/// every scan.  The Home is obtained at call time so Reconnect/Restore never
/// leave this reader pointing at a stale path.
pub struct SqliteGitSourceCapabilityReader {
    home_context: Arc<WriteGate>,
    catalog_file_name: String,
    filesystem: Arc<dyn FileSystem>,
}

impl SqliteGitSourceCapabilityReader {
    pub fn new(
        home_context: Arc<WriteGate>,
        catalog_file_name: impl Into<String>,
        filesystem: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            home_context,
            catalog_file_name: catalog_file_name.into(),
            filesystem,
        }
    }
}

impl GitSourceCapabilityReader for SqliteGitSourceCapabilityReader {
    fn read(&self) -> Result<GitSourceCapabilityFacts, String> {
        let home = self
            .home_context
            .bound_home()
            .map_err(|error| error.to_string())?;
        let catalog_path = home.path.join(&self.catalog_file_name);
        let connection = Connection::open_with_flags(
            &catalog_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("open {} read-only: {error}", catalog_path.display()))?;

        let tables = table_names(&connection)?;
        let catalog_structure = catalog_structure(&connection, &tables)?;
        let sources = read_sources(
            &connection,
            &tables,
            &catalog_structure,
            self.filesystem.as_ref(),
            &home.path.join("remotes"),
        )?;
        Ok(GitSourceCapabilityFacts {
            catalog_structure,
            sources,
        })
    }
}

fn table_names(connection: &Connection) -> Result<HashSet<String>, String> {
    connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|error| error.to_string())
}

fn catalog_structure(
    connection: &Connection,
    tables: &HashSet<String>,
) -> Result<GitSourceCatalogStructure, String> {
    let has_parents_table = tables.contains("remote_source_parents");
    let has_aliases_table = tables.contains("remote_source_aliases");
    let has_repository_sources_table = tables.contains("git_repository_sources");
    let has_releases_table = tables.contains("git_source_releases");
    let has_release_members_table = tables.contains("git_source_release_members");
    let has_members_table = tables.contains("git_source_members");
    let target_tables_present = has_parents_table
        && has_aliases_table
        && has_repository_sources_table
        && has_releases_table
        && has_release_members_table
        && has_members_table;

    let has_required_columns = target_tables_present
        && table_has_columns(
            connection,
            "remote_source_parents",
            &[
                ("remote_id", true),
                ("canonical_url", true),
                ("created_at", true),
            ],
        )?
        && table_has_columns(
            connection,
            "remote_source_aliases",
            &[
                ("remote_id", true),
                ("alias_url", true),
                ("confirmed_at", true),
            ],
        )?
        && table_has_columns(
            connection,
            "git_repository_sources",
            &[
                ("remote_id", true),
                ("provider", true),
                ("canonical_url", true),
                ("tracking_mode", true),
                ("tracking_value", false),
                ("current_selected_ref", false),
                ("current_release_id", false),
                ("created_at", true),
                ("updated_at", true),
            ],
        )?
        && table_has_columns(
            connection,
            "git_source_releases",
            &[
                ("release_id", true),
                ("remote_id", true),
                ("selection_kind", true),
                ("selected_ref", true),
                ("resolved_commit", true),
                ("discovered_at", true),
            ],
        )?
        && table_has_columns(
            connection,
            "git_source_release_members",
            &[
                ("release_id", true),
                ("skill_id", true),
                ("skill_path", true),
                ("directory_name", true),
                ("directory_identity_key", true),
                ("tree_hash", true),
                ("provider_hash", false),
            ],
        )?
        && table_has_columns(
            connection,
            "git_source_members",
            &[
                ("skill_id", true),
                ("remote_id", true),
                ("skill_path", true),
                ("storage_relpath", true),
                ("presence", true),
                ("first_seen_release_id", true),
                ("last_seen_release_id", true),
                ("last_checked_at", false),
                ("last_updated_at", false),
            ],
        )?;

    let has_required_foreign_keys = target_tables_present
        && has_foreign_key(
            connection,
            "remote_source_aliases",
            "remote_id",
            "remote_source_parents",
            "remote_id",
        )?
        && has_foreign_key(
            connection,
            "git_repository_sources",
            "remote_id",
            "remote_source_parents",
            "remote_id",
        )?
        && has_foreign_key(
            connection,
            "git_repository_sources",
            "current_release_id",
            "git_source_releases",
            "release_id",
        )?
        && has_foreign_key(
            connection,
            "git_source_releases",
            "remote_id",
            "remote_source_parents",
            "remote_id",
        )?
        && has_foreign_key(
            connection,
            "git_source_release_members",
            "release_id",
            "git_source_releases",
            "release_id",
        )?
        && has_foreign_key(
            connection,
            "git_source_release_members",
            "skill_id",
            "skills",
            "id",
        )?
        && has_foreign_key(connection, "git_source_members", "skill_id", "skills", "id")?
        && has_foreign_key(
            connection,
            "git_source_members",
            "remote_id",
            "remote_source_parents",
            "remote_id",
        )?
        && has_foreign_key(
            connection,
            "git_source_members",
            "first_seen_release_id",
            "git_source_releases",
            "release_id",
        )?
        && has_foreign_key(
            connection,
            "git_source_members",
            "last_seen_release_id",
            "git_source_releases",
            "release_id",
        )?;

    let has_required_unique_constraints = target_tables_present
        && has_unique_columns(connection, "remote_source_parents", &["remote_id"])?
        && has_unique_columns(connection, "remote_source_parents", &["canonical_url"])?
        && has_unique_columns(
            connection,
            "remote_source_aliases",
            &["remote_id", "alias_url"],
        )?
        && has_unique_columns(connection, "remote_source_aliases", &["alias_url"])?
        && has_unique_columns(connection, "git_repository_sources", &["remote_id"])?
        && has_unique_columns(
            connection,
            "git_repository_sources",
            &["provider", "canonical_url"],
        )?
        && has_unique_columns(connection, "git_source_releases", &["release_id"])?
        && has_unique_columns(
            connection,
            "git_source_releases",
            &["remote_id", "selected_ref", "resolved_commit"],
        )?
        && has_unique_columns(
            connection,
            "git_source_release_members",
            &["release_id", "skill_path"],
        )?
        && has_unique_columns(
            connection,
            "git_source_release_members",
            &["release_id", "skill_id"],
        )?
        && has_unique_columns(
            connection,
            "git_source_members",
            &["remote_id", "skill_path"],
        )?
        && has_unique_columns(
            connection,
            "git_source_members",
            &["remote_id", "storage_relpath"],
        )?
        && has_unique_columns(connection, "git_source_members", &["skill_id"])?;

    Ok(GitSourceCatalogStructure {
        has_repository_sources_table,
        has_releases_table,
        has_release_members_table,
        has_members_table,
        has_required_columns,
        has_required_foreign_keys,
        has_required_unique_constraints,
        has_clean_foreign_key_check: foreign_key_check_is_clean(connection)?,
        has_clean_integrity_check: integrity_check_is_clean(connection)?,
    })
}

fn foreign_key_check_is_clean(connection: &Connection) -> Result<bool, String> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|error| error.to_string())?;
    let mut rows = statement.query([]).map_err(|error| error.to_string())?;
    Ok(rows.next().map_err(|error| error.to_string())?.is_none())
}

fn integrity_check_is_clean(connection: &Connection) -> Result<bool, String> {
    let checks = connection
        .prepare("PRAGMA integrity_check")
        .map_err(|error| error.to_string())?
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(checks.as_slice() == ["ok"])
}

fn table_has_columns(
    connection: &Connection,
    table: &str,
    required: &[(&str, bool)],
) -> Result<bool, String> {
    let sql = format!("PRAGMA table_info({})", quoted_identifier(table));
    let columns = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                (row.get::<_, i64>(3)? != 0, row.get::<_, i64>(5)? != 0),
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(required.iter().all(|(name, requires_not_null)| {
        columns
            .get(*name)
            .is_some_and(|(not_null, primary_key)| !requires_not_null || *not_null || *primary_key)
    }))
}

fn has_foreign_key(
    connection: &Connection,
    table: &str,
    from: &str,
    target_table: &str,
    to: &str,
) -> Result<bool, String> {
    let sql = format!("PRAGMA foreign_key_list({})", quoted_identifier(table));
    let matches = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(3)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(matches
        .iter()
        .any(|(actual_from, actual_table, actual_to)| {
            actual_from == from && actual_table == target_table && actual_to == to
        }))
}

fn has_unique_columns(
    connection: &Connection,
    table: &str,
    expected: &[&str],
) -> Result<bool, String> {
    let sql = format!("PRAGMA index_list({})", quoted_identifier(table));
    let indexes = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    for (index_name, unique) in indexes {
        if !unique {
            continue;
        }
        let sql = format!("PRAGMA index_info({})", quoted_identifier(&index_name));
        let columns = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(2)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut columns = columns;
        columns.sort_by_key(|(position, _)| *position);
        if columns
            .iter()
            .map(|(_, name)| name.as_str())
            .eq(expected.iter().copied())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_sources(
    connection: &Connection,
    tables: &HashSet<String>,
    structure: &GitSourceCatalogStructure,
    filesystem: &dyn FileSystem,
    remotes_root: &Path,
) -> Result<Vec<GitSourceFact>, String> {
    if !tables.contains("remote_source_parents") {
        return Ok(Vec::new());
    }
    let parents = connection
        .prepare(
            "SELECT remote_id, canonical_url
             FROM remote_source_parents
             ORDER BY canonical_url, remote_id",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;

    parents
        .into_iter()
        .map(|(remote_id, canonical_url)| {
            let catalog_aliases = if tables.contains("remote_source_aliases") {
                connection
                    .prepare(
                        "SELECT alias_url FROM remote_source_aliases
                         WHERE remote_id = ?1 ORDER BY alias_url",
                    )
                    .map_err(|error| error.to_string())?
                    .query_map(params![&remote_id], |row| row.get::<_, String>(0))
                    .map_err(|error| error.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?
            } else {
                // The structure fact fails closed, so this value cannot make
                // an incomplete Catalog eligible for repository-source use.
                Vec::new()
            };
            let repository = structure
                .supports_repository_sources()
                .then(|| read_repository_source(connection, &remote_id))
                .transpose()?
                .flatten();
            let manifest = match filesystem.read_remote_parent_manifest(remotes_root, &remote_id) {
                Ok(None) => GitSourceManifestFact::Missing,
                Ok(Some(manifest)) => GitSourceManifestFact::Present {
                    member_plugins: Box::new(manifest.member_plugins),
                    remote_id: manifest.remote_id,
                    canonical_url: manifest.canonical_url,
                    aliases: manifest.aliases,
                    provider: manifest.provider,
                    // Version is never the eligibility gate (spec §3.4 v7):
                    // a manifest without the tracking facts is simply
                    // incomplete capability evidence, so it stays Legacy.
                    tracking_mode: manifest.tracking_mode,
                    tracking_value: manifest.tracking_value,
                    current_selected_ref: manifest.current_selected_ref,
                    current_release_id: manifest.current_release_id,
                },
                Err(_) => GitSourceManifestFact::Unreadable,
            };
            Ok(GitSourceFact {
                remote_id,
                canonical_url,
                catalog_aliases,
                repository,
                manifest,
            })
        })
        .collect()
}

fn read_repository_source(
    connection: &Connection,
    remote_id: &str,
) -> Result<Option<GitRepositorySourceFact>, String> {
    let repository = connection
        .query_row(
            "SELECT provider, canonical_url, tracking_mode, tracking_value,
                    current_selected_ref, current_release_id
             FROM git_repository_sources WHERE remote_id = ?1",
            [remote_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((
        provider,
        canonical_url,
        tracking_mode,
        tracking_value,
        current_selected_ref,
        current_release_id,
    )) = repository
    else {
        return Ok(None);
    };
    let current_release = current_release_id
        .as_deref()
        .map(|release_id| read_release(connection, release_id))
        .transpose()?
        .flatten();
    let current_members = connection
        .prepare(
            "SELECT skill_id, skill_path, storage_relpath, presence
             FROM git_source_members
             WHERE remote_id = ?1 ORDER BY skill_path",
        )
        .map_err(|error| error.to_string())?
        .query_map(params![remote_id], |row| {
            Ok(GitSourceMemberFact {
                skill_id: row.get(0)?,
                skill_path: row.get(1)?,
                storage_relpath: row.get(2)?,
                presence: row.get::<_, String>(3)? == "current",
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(Some(GitRepositorySourceFact {
        provider,
        canonical_url,
        tracking_mode,
        tracking_value,
        current_selected_ref,
        current_release_id,
        current_release,
        current_members,
    }))
}

fn read_release(
    connection: &Connection,
    release_id: &str,
) -> Result<Option<GitSourceReleaseFact>, String> {
    let release = connection
        .query_row(
            "SELECT remote_id, selection_kind, selected_ref, resolved_commit
             FROM git_source_releases WHERE release_id = ?1",
            [release_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((remote_id, selection_kind, selected_ref, resolved_commit)) = release else {
        return Ok(None);
    };
    let member_paths = connection
        .prepare(
            "SELECT skill_path FROM git_source_release_members
             WHERE release_id = ?1 ORDER BY skill_path",
        )
        .map_err(|error| error.to_string())?
        .query_map([release_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(Some(GitSourceReleaseFact {
        release_id: release_id.into(),
        remote_id,
        selection_kind,
        selected_ref,
        resolved_commit,
        member_paths,
    }))
}

fn quoted_identifier(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
