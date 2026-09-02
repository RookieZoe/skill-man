use std::path::PathBuf;

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::SqliteCatalogStore;
use crate::core::agent_configuration::{AgentConfigurationOrigin, AgentRootRole, Compatibility};
use crate::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, AgentConfigurationWrite, RecentProjectFolder,
    StoredAgentConfiguration, StoredAgentRootMembership, StoredGlobalSkillRoot,
};

impl AgentConfigurationStore for SqliteCatalogStore {
    fn agent_configuration_snapshot(
        &self,
    ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| unavailable("SQLite lock poisoned"))?;
        let snapshot_version = read_snapshot_version(&connection)?;
        let configurations = {
            let mut statement = connection
                .prepare(
                    "SELECT agent_id, origin, preset_key, name, name_identity_key,
                            compatibility, project_skills_dir, created_at, updated_at
                     FROM agent_configurations
                     ORDER BY name_identity_key, agent_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(StoredAgentConfiguration {
                        agent_id: row.get(0)?,
                        origin: parse_origin(&row.get::<_, String>(1)?)?,
                        preset_key: row.get(2)?,
                        name: row.get(3)?,
                        name_identity_key: row.get(4)?,
                        compatibility: parse_compatibility(&row.get::<_, String>(5)?)?,
                        project_skills_dir: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                        created_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        memberships: Vec::new(),
                    })
                })
                .map_err(sqlite_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
        };
        let mut configurations = configurations;
        for configuration in &mut configurations {
            let mut statement = connection
                .prepare(
                    "SELECT root_id, role
                     FROM agent_global_roots
                     WHERE agent_id = ?1
                     ORDER BY CASE role WHEN 'activation_target' THEN 0 ELSE 1 END, root_id",
                )
                .map_err(sqlite_error)?;
            configuration.memberships = statement
                .query_map([&configuration.agent_id], |row| {
                    Ok(StoredAgentRootMembership {
                        root_id: row.get(0)?,
                        role: parse_role(&row.get::<_, String>(1)?)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
        }

        let roots = {
            let mut statement = connection
                .prepare(
                    "SELECT root_id, configured_path, path_identity_key
                     FROM global_skill_roots
                     ORDER BY path_identity_key, root_id",
                )
                .map_err(sqlite_error)?;
            statement
                .query_map([], |row| {
                    Ok(StoredGlobalSkillRoot {
                        root_id: row.get(0)?,
                        configured_path: PathBuf::from(row.get::<_, String>(1)?),
                        path_identity_key: row.get(2)?,
                        consumer_agent_ids: Vec::new(),
                        activation_skill_ids: Vec::new(),
                    })
                })
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?
        };
        let mut roots = roots;
        for root in &mut roots {
            root.consumer_agent_ids = connection
                .prepare(
                    "SELECT agent_id FROM agent_global_roots
                     WHERE root_id = ?1 ORDER BY agent_id",
                )
                .map_err(sqlite_error)?
                .query_map([&root.root_id], |row| row.get(0))
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
            root.activation_skill_ids = connection
                .prepare(
                    "SELECT skill_id FROM activations
                     WHERE target_root_id = ?1 ORDER BY skill_id",
                )
                .map_err(sqlite_error)?
                .query_map([&root.root_id], |row| row.get(0))
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
        }
        Ok(AgentConfigurationStoreSnapshot {
            snapshot_version,
            configurations,
            roots,
        })
    }

    fn apply_agent_configuration_change(
        &self,
        expected_snapshot_version: u64,
        change: AgentConfigurationStoreChange,
    ) -> Result<u64, AgentConfigurationStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| unavailable("SQLite lock poisoned"))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if read_snapshot_version(&transaction)? != expected_snapshot_version {
            return Err(AgentConfigurationStoreError::Stale);
        }

        match change {
            AgentConfigurationStoreChange::Create(write) => {
                if agent_exists(&transaction, &write.agent_id)? {
                    return Err(AgentConfigurationStoreError::Stale);
                }
                validate_write(&transaction, &write)?;
                insert_configuration(&transaction, &write)?;
                replace_memberships(&transaction, &write)?;
            }
            AgentConfigurationStoreChange::Edit(write) => {
                if !agent_exists(&transaction, &write.agent_id)? {
                    return Err(AgentConfigurationStoreError::NotFound {
                        agent_id: write.agent_id,
                    });
                }
                validate_write(&transaction, &write)?;
                guard_removed_target(&transaction, &write.agent_id, Some(&write))?;
                update_configuration(&transaction, &write)?;
                replace_memberships(&transaction, &write)?;
            }
            AgentConfigurationStoreChange::Delete { agent_id } => {
                if !agent_exists(&transaction, &agent_id)? {
                    return Err(AgentConfigurationStoreError::NotFound { agent_id });
                }
                guard_removed_target(&transaction, &agent_id, None)?;
                transaction
                    .execute(
                        "DELETE FROM agent_configurations WHERE agent_id = ?1",
                        [&agent_id],
                    )
                    .map_err(sqlite_error)?;
            }
        }
        delete_orphan_roots(&transaction)?;
        let next = increment_snapshot_version(&transaction, expected_snapshot_version)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(next)
    }

    fn list_recent_project_folders(
        &self,
    ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| unavailable("SQLite lock poisoned"))?;
        connection
            .prepare(
                "SELECT canonical_path_key, canonical_path, last_used_at
                 FROM recent_project_folders
                 ORDER BY last_used_at DESC, canonical_path_key DESC
                 LIMIT 10",
            )
            .map_err(sqlite_error)?
            .query_map([], |row| {
                Ok(RecentProjectFolder {
                    canonical_path_key: row.get(0)?,
                    canonical_path: PathBuf::from(row.get::<_, String>(1)?),
                    last_used_at: row.get(2)?,
                })
            })
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)
    }

    fn record_recent_project_folder(
        &self,
        folder: RecentProjectFolder,
    ) -> Result<(), AgentConfigurationStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| unavailable("SQLite lock poisoned"))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO recent_project_folders (
                    canonical_path_key, canonical_path, last_used_at
                 ) VALUES (?1, ?2, ?3)
                 ON CONFLICT(canonical_path_key) DO UPDATE SET
                    canonical_path = excluded.canonical_path,
                    last_used_at = excluded.last_used_at",
                params![
                    folder.canonical_path_key,
                    folder.canonical_path.to_string_lossy(),
                    folder.last_used_at
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "DELETE FROM recent_project_folders
                 WHERE canonical_path_key IN (
                    SELECT canonical_path_key
                    FROM recent_project_folders
                    ORDER BY last_used_at DESC, canonical_path_key DESC
                    LIMIT -1 OFFSET 10
                 )",
                [],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "UPDATE catalog_meta
                 SET snapshot_version = snapshot_version + 1
                 WHERE singleton = 1",
                [],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)
    }

    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| unavailable("SQLite lock poisoned"))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute("DELETE FROM recent_project_folders", [])
            .map_err(sqlite_error)?;
        if changed != 0 {
            transaction
                .execute(
                    "UPDATE catalog_meta
                     SET snapshot_version = snapshot_version + 1
                     WHERE singleton = 1",
                    [],
                )
                .map_err(sqlite_error)?;
        }
        transaction.commit().map_err(sqlite_error)
    }
}

fn validate_write(
    transaction: &Transaction<'_>,
    write: &AgentConfigurationWrite,
) -> Result<(), AgentConfigurationStoreError> {
    if write.roots.is_empty()
        || write
            .roots
            .iter()
            .filter(|root| root.role == AgentRootRole::ActivationTarget)
            .count()
            != 1
    {
        return Err(AgentConfigurationStoreError::InvalidTargetMembership);
    }
    let duplicate_root_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM agent_configurations
             WHERE name_identity_key = ?1 AND agent_id != ?2",
            params![write.name_identity_key, write.agent_id],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if duplicate_root_count != 0 {
        return Err(AgentConfigurationStoreError::NameConflict);
    }
    let mut root_ids = std::collections::BTreeSet::new();
    let mut identities = std::collections::BTreeSet::new();
    for root in &write.roots {
        if !root_ids.insert(&root.root_id) || !identities.insert(&root.path_identity_key) {
            return Err(AgentConfigurationStoreError::RootConflict);
        }
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT root_id, path_identity_key
                 FROM global_skill_roots
                 WHERE root_id = ?1 OR path_identity_key = ?2",
                params![root.root_id, root.path_identity_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?;
        if existing.is_some_and(|(root_id, identity)| {
            root_id != root.root_id || identity != root.path_identity_key
        }) {
            return Err(AgentConfigurationStoreError::RootConflict);
        }
    }
    Ok(())
}

fn insert_configuration(
    transaction: &Transaction<'_>,
    write: &AgentConfigurationWrite,
) -> Result<(), AgentConfigurationStoreError> {
    transaction
        .execute(
            "INSERT INTO agent_configurations (
                agent_id, origin, preset_key, name, name_identity_key,
                compatibility, project_skills_dir, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                write.agent_id,
                origin_value(write.origin),
                write.preset_key,
                write.name,
                write.name_identity_key,
                compatibility_value(write.compatibility),
                write
                    .project_skills_dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                write.created_at,
                write.updated_at
            ],
        )
        .map_err(classify_constraint)?;
    Ok(())
}

fn update_configuration(
    transaction: &Transaction<'_>,
    write: &AgentConfigurationWrite,
) -> Result<(), AgentConfigurationStoreError> {
    transaction
        .execute(
            "UPDATE agent_configurations SET
                origin = ?2,
                preset_key = ?3,
                name = ?4,
                name_identity_key = ?5,
                compatibility = ?6,
                project_skills_dir = ?7,
                updated_at = ?8
             WHERE agent_id = ?1",
            params![
                write.agent_id,
                origin_value(write.origin),
                write.preset_key,
                write.name,
                write.name_identity_key,
                compatibility_value(write.compatibility),
                write
                    .project_skills_dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                write.updated_at
            ],
        )
        .map_err(classify_constraint)?;
    Ok(())
}

fn replace_memberships(
    transaction: &Transaction<'_>,
    write: &AgentConfigurationWrite,
) -> Result<(), AgentConfigurationStoreError> {
    transaction
        .execute(
            "DELETE FROM agent_global_roots WHERE agent_id = ?1",
            [&write.agent_id],
        )
        .map_err(sqlite_error)?;
    for root in &write.roots {
        transaction
            .execute(
                "INSERT INTO global_skill_roots (
                    root_id, configured_path, path_identity_key, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(path_identity_key) DO NOTHING",
                params![
                    root.root_id,
                    root.configured_path.to_string_lossy(),
                    root.path_identity_key,
                    write.created_at,
                    write.updated_at
                ],
            )
            .map_err(classify_constraint)?;
        let persisted_root_id: String = transaction
            .query_row(
                "SELECT root_id FROM global_skill_roots WHERE path_identity_key = ?1",
                [&root.path_identity_key],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if persisted_root_id != root.root_id {
            return Err(AgentConfigurationStoreError::RootConflict);
        }
        transaction
            .execute(
                "INSERT INTO agent_global_roots (agent_id, root_id, role)
                 VALUES (?1, ?2, ?3)",
                params![write.agent_id, root.root_id, role_value(root.role)],
            )
            .map_err(classify_constraint)?;
    }
    Ok(())
}

fn guard_removed_target(
    transaction: &Transaction<'_>,
    agent_id: &str,
    replacement: Option<&AgentConfigurationWrite>,
) -> Result<(), AgentConfigurationStoreError> {
    let old_target: Option<String> = transaction
        .query_row(
            "SELECT root_id FROM agent_global_roots
             WHERE agent_id = ?1 AND role = 'activation_target'",
            [agent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    let Some(old_target) = old_target else {
        return Err(AgentConfigurationStoreError::InvalidTargetMembership);
    };
    let retains_target = replacement.is_some_and(|write| {
        write
            .roots
            .iter()
            .any(|root| root.role == AgentRootRole::ActivationTarget && root.root_id == old_target)
    });
    if retains_target {
        return Ok(());
    }
    let reference_count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM agent_global_roots
             WHERE root_id = ?1 AND role = 'activation_target'",
            [&old_target],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    if reference_count != 1 {
        return Ok(());
    }
    let skill_ids = transaction
        .prepare(
            "SELECT skill_id FROM activations
             WHERE target_root_id = ?1 ORDER BY skill_id",
        )
        .map_err(sqlite_error)?
        .query_map([&old_target], |row| row.get(0))
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    if skill_ids.is_empty() {
        Ok(())
    } else {
        Err(AgentConfigurationStoreError::TargetInUse { skill_ids })
    }
}

fn delete_orphan_roots(transaction: &Transaction<'_>) -> Result<(), AgentConfigurationStoreError> {
    transaction
        .execute(
            "DELETE FROM global_skill_roots
             WHERE NOT EXISTS (
                SELECT 1 FROM agent_global_roots
                WHERE agent_global_roots.root_id = global_skill_roots.root_id
             )
             AND NOT EXISTS (
                SELECT 1 FROM activations
                WHERE activations.target_root_id = global_skill_roots.root_id
             )",
            [],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn increment_snapshot_version(
    transaction: &Transaction<'_>,
    expected: u64,
) -> Result<u64, AgentConfigurationStoreError> {
    let expected_sql =
        i64::try_from(expected).map_err(|_| unavailable("Catalog snapshot version overflow"))?;
    let changed = transaction
        .execute(
            "UPDATE catalog_meta
             SET snapshot_version = snapshot_version + 1
             WHERE singleton = 1 AND snapshot_version = ?1",
            [expected_sql],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(AgentConfigurationStoreError::Stale);
    }
    expected
        .checked_add(1)
        .ok_or_else(|| unavailable("Catalog snapshot version overflow"))
}

fn read_snapshot_version(
    connection: &rusqlite::Connection,
) -> Result<u64, AgentConfigurationStoreError> {
    let value: i64 = connection
        .query_row(
            "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(sqlite_error)?;
    u64::try_from(value).map_err(|_| unavailable("invalid Catalog snapshot version"))
}

fn agent_exists(
    transaction: &Transaction<'_>,
    agent_id: &str,
) -> Result<bool, AgentConfigurationStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agent_configurations WHERE agent_id = ?1
             )",
            [agent_id],
            |row| row.get(0),
        )
        .map_err(sqlite_error)
}

fn origin_value(value: AgentConfigurationOrigin) -> &'static str {
    match value {
        AgentConfigurationOrigin::Preset => "preset",
        AgentConfigurationOrigin::Custom => "custom",
    }
}

fn role_value(value: AgentRootRole) -> &'static str {
    match value {
        AgentRootRole::ScanOnly => "scan_only",
        AgentRootRole::ActivationTarget => "activation_target",
    }
}

fn compatibility_value(value: Compatibility) -> &'static str {
    match value {
        Compatibility::Verified => "verified",
        Compatibility::Unknown => "unknown",
    }
}

fn parse_origin(value: &str) -> rusqlite::Result<AgentConfigurationOrigin> {
    match value {
        "preset" => Ok(AgentConfigurationOrigin::Preset),
        "custom" => Ok(AgentConfigurationOrigin::Custom),
        _ => Err(invalid_enum(value)),
    }
}

fn parse_role(value: &str) -> rusqlite::Result<AgentRootRole> {
    match value {
        "scan_only" => Ok(AgentRootRole::ScanOnly),
        "activation_target" => Ok(AgentRootRole::ActivationTarget),
        _ => Err(invalid_enum(value)),
    }
}

fn parse_compatibility(value: &str) -> rusqlite::Result<Compatibility> {
    match value {
        "verified" => Ok(Compatibility::Verified),
        "unknown" => Ok(Compatibility::Unknown),
        _ => Err(invalid_enum(value)),
    }
}

fn invalid_enum(value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid persisted Agent Configuration enum '{value}'").into(),
    )
}

fn classify_constraint(error: rusqlite::Error) -> AgentConfigurationStoreError {
    let detail = error.to_string();
    if detail.contains("agent_configurations.name_identity_key") {
        AgentConfigurationStoreError::NameConflict
    } else if detail.contains("global_skill_roots.path_identity_key")
        || detail.contains("global_skill_roots.root_id")
    {
        AgentConfigurationStoreError::RootConflict
    } else if detail.contains("one_activation_target_per_agent") {
        AgentConfigurationStoreError::InvalidTargetMembership
    } else {
        sqlite_error(error)
    }
}

fn sqlite_error(error: rusqlite::Error) -> AgentConfigurationStoreError {
    AgentConfigurationStoreError::Unavailable(error.to_string())
}

fn unavailable(message: impl Into<String>) -> AgentConfigurationStoreError {
    AgentConfigurationStoreError::Unavailable(message.into())
}
