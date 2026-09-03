//! SQLite implementation of the v9 Source lifecycle seam (ticket #93).
//!
//! Every method re-reads complete source facts; the Core never passes
//! member/path facts from a client. All commits are one SQLite transaction.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::Transaction;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::SqliteCatalogStore;
use crate::core::domain::{Health, SkillId};
use crate::seams::source_update_store::{
    LocalSourceCopyRecord, SourceMemberPresence, SourceRemoveFacts, SourceUpdateCurrentMember,
    SourceUpdateCurrentSource, SourceUpdateRecord, SourceUpdateStore, SourceUpdateStoreError,
};

type LiveMemberRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);
// skill_id, directory_name, identity_key, display_name, description, health,
// skill_path, storage_relpath, presence, last_seen_release_id

fn update_sql_error(error: rusqlite::Error) -> SourceUpdateStoreError {
    SourceUpdateStoreError::Unavailable(error.to_string())
}

fn presence_value(presence: SourceMemberPresence) -> &'static str {
    match presence {
        SourceMemberPresence::Current => "current",
        SourceMemberPresence::Absent => "absent",
    }
}

fn parse_presence(value: &str) -> SourceMemberPresence {
    match value {
        "absent" => SourceMemberPresence::Absent,
        _ => SourceMemberPresence::Current,
    }
}

/// Complete live member rows of one source (current + tombstoned).
fn read_live_members(
    connection: &Connection,
    remote_id: &str,
) -> Result<Vec<LiveMemberRow>, SourceUpdateStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT skills.id, skills.directory_name, skills.directory_identity_key,
                    skills.display_name, skills.description, skills.health,
                    members.skill_path, members.storage_relpath, members.presence,
                    members.last_seen_release_id
             FROM git_source_members members
             JOIN skills ON skills.id = members.skill_id
             WHERE members.remote_id = ?1
             ORDER BY members.skill_path, skills.id",
        )
        .map_err(update_sql_error)?;
    statement
        .query_map([remote_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })
        .map_err(update_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(update_sql_error)
}

/// `(skill_path, tree_hash)` of the current Source Release.
fn read_release_trees(
    connection: &Connection,
    release_id: &str,
) -> Result<Vec<(String, String)>, SourceUpdateStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT skill_path, tree_hash FROM git_source_release_members
              WHERE release_id = ?1 ORDER BY skill_path",
        )
        .map_err(update_sql_error)?;
    statement
        .query_map([release_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(update_sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(update_sql_error)
}

type SourceRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    String,
);

fn read_source_row(
    connection: &Connection,
    remote_id: &str,
) -> Result<Option<SourceRow>, SourceUpdateStoreError> {
    connection
        .query_row(
            "SELECT source.remote_id, source.provider, source.canonical_url,
                    source.tracking_mode, source.tracking_value,
                    source.current_selected_ref, source.current_release_id,
                    parent.created_at
               FROM git_repository_sources source
               JOIN remote_source_parents parent ON parent.remote_id = source.remote_id
              WHERE source.remote_id = ?1",
            [remote_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(update_sql_error)
}

/// One member row projected to `(skill_id, directory_name, identity_key,
/// display_name, description, health, skill_path, storage_relpath,
/// presence, tree_hash)` — the shared row shape of the frozen previous and
/// live member projections.
#[allow(clippy::type_complexity)]
type SourceMemberRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

/// Frozen previous member rows sorted by the same key as live rows.
fn sorted_previous_members(record: &SourceUpdateRecord) -> Vec<SourceMemberRow> {
    let mut rows = record
        .previous_members
        .iter()
        .map(|member| {
            (
                member.skill_id.0.clone(),
                member.directory_name.clone(),
                member.identity_key.clone(),
                member.display_name.clone(),
                member.description.clone(),
                health_text(member.health).to_string(),
                member.skill_path.clone(),
                member.storage_relpath.clone(),
                presence_value(member.presence).to_string(),
                member.tree_hash.clone(),
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        (left.6.as_str(), left.0.as_str()).cmp(&(right.6.as_str(), right.0.as_str()))
    });
    rows
}

fn health_text(health: Health) -> &'static str {
    match health {
        Health::Healthy => "healthy",
        Health::Broken => "broken",
        Health::Modified => "modified",
        Health::SourceSnapshotMismatch => "source_snapshot_mismatch",
    }
}

fn parse_health_row(value: &str) -> Health {
    match value {
        "broken" => Health::Broken,
        "modified" => Health::Modified,
        "source_snapshot_mismatch" => Health::SourceSnapshotMismatch,
        _ => Health::Healthy,
    }
}

/// Live rows in the same projection as `sorted_previous_members`.
#[allow(clippy::type_complexity)]
fn sorted_live_rows(
    connection: &Connection,
    remote_id: &str,
    release_trees: &[(String, String)],
) -> Result<Vec<SourceMemberRow>, SourceUpdateStoreError> {
    let trees = release_trees
        .iter()
        .map(|(path, tree)| (path.as_str(), tree.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut rows = read_live_members(connection, remote_id)?
        .into_iter()
        .map(|member| {
            let tree = if member.8 == "current" {
                trees.get(member.6.as_str()).map(|tree| (*tree).to_string())
            } else {
                None
            };
            (
                member.0, member.1, member.2, member.3, member.4, member.5, member.6, member.7,
                member.8, tree,
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        (left.6.as_str(), left.0.as_str()).cmp(&(right.6.as_str(), right.0.as_str()))
    });
    Ok(rows)
}

/// Prove live state still exactly equals the frozen previous facts and the
/// target release can commit.
fn validate_source_update_records(
    connection: &Connection,
    record: &SourceUpdateRecord,
) -> Result<(), SourceUpdateStoreError> {
    let some_source = read_source_row(connection, &record.remote_id)?;
    let source = some_source.ok_or_else(|| {
        SourceUpdateStoreError::Conflict("the Git Repository Source is no longer complete".into())
    })?;
    if source.1 != record.provider
        || source.2 != record.canonical_url
        || source.3 != record.previous_tracking_mode
        || source.4 != record.previous_tracking_value
        || source.5 != record.previous_selected_ref
        || source.6.as_deref() != Some(record.previous_release_id.as_str())
    {
        return Err(SourceUpdateStoreError::Conflict(
            "the source facts changed after the Update plan was frozen".into(),
        ));
    }
    let previous_commit: Option<String> = connection
        .query_row(
            "SELECT resolved_commit FROM git_source_releases
              WHERE release_id = ?1 AND remote_id = ?2",
            params![record.previous_release_id, record.remote_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(update_sql_error)?;
    if previous_commit.as_deref() != Some(record.previous_resolved_commit.as_str()) {
        return Err(SourceUpdateStoreError::Conflict(
            "the current Source Release is missing or changed".into(),
        ));
    }
    let release_trees = read_release_trees(connection, &record.previous_release_id)?;
    let live = sorted_live_rows(connection, &record.remote_id, &release_trees)?;
    let expected = sorted_previous_members(record);
    if live != expected {
        return Err(SourceUpdateStoreError::Conflict(
            "the complete current member set changed after the Update plan was frozen".into(),
        ));
    }
    if live
        .iter()
        .any(|member| member.5 == "source_snapshot_mismatch")
    {
        return Err(SourceUpdateStoreError::Conflict(
            "the source has a snapshot mismatch; restore the current release before Update".into(),
        ));
    }
    if record.members.is_empty() {
        return Err(SourceUpdateStoreError::Conflict(
            "a Source Update must contain the complete target release".into(),
        ));
    }
    let mut seen_ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    let mut seen_identities = BTreeSet::new();
    for member in &record.members {
        if !seen_ids.insert(member.skill_id.0.as_str())
            || !seen_paths.insert(member.skill_path.as_str())
            || !seen_identities.insert(member.identity_key.as_str())
        {
            return Err(SourceUpdateStoreError::Conflict(
                "the target release carries duplicate members".into(),
            ));
        }
        let live_member = read_live_members(connection, &record.remote_id)?
            .into_iter()
            .find(|row| row.0 == member.skill_id.0);
        match member.origin {
            crate::seams::source_update_store::SourceUpdateMemberOrigin::Existing => {
                let Some(existing) = live_member else {
                    return Err(SourceUpdateStoreError::Conflict(format!(
                        "the frozen member '{}' no longer exists",
                        member.skill_path
                    )));
                };
                if existing.6 != member.skill_path || existing.7 != member.storage_relpath {
                    return Err(SourceUpdateStoreError::Conflict(format!(
                        "the frozen member '{}' changed its path or namespace",
                        member.skill_path
                    )));
                }
            }
            crate::seams::source_update_store::SourceUpdateMemberOrigin::New => {
                if live_member.is_some() {
                    return Err(SourceUpdateStoreError::Conflict(
                        "a New member id is already owned by this source".into(),
                    ));
                }
                let skill_exists: bool = connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM skills WHERE id = ?1)",
                        [&member.skill_id.0],
                        |row| row.get(0),
                    )
                    .map_err(update_sql_error)?;
                if skill_exists {
                    return Err(SourceUpdateStoreError::Conflict(format!(
                        "the New member id for '{}' is already used",
                        member.skill_path
                    )));
                }
            }
        }
    }
    for removed in &record.removed_members {
        let rows = read_live_members(connection, &record.remote_id)?;
        let Some(existing) = rows.iter().find(|row| row.0 == removed.skill_id.0) else {
            return Err(SourceUpdateStoreError::Conflict(
                "a removed member no longer exists".into(),
            ));
        };
        if existing.8 != "current"
            || existing.6 != removed.skill_path
            || existing.7 != removed.storage_relpath
        {
            return Err(SourceUpdateStoreError::Conflict(
                "a removed member is no longer current".into(),
            ));
        }
        let current_tree = release_trees
            .iter()
            .find(|(path, _)| path == &removed.skill_path)
            .map(|(_, tree)| tree.clone())
            .ok_or_else(|| {
                SourceUpdateStoreError::Conflict(
                    "a removed member has no current release tree facts".into(),
                )
            })?;
        if current_tree != removed.previous_tree_hash {
            return Err(SourceUpdateStoreError::Conflict(
                "a removed member's frozen tree facts changed".into(),
            ));
        }
    }
    Ok(())
}

fn commit_update_members(
    transaction: &Transaction,
    record: &SourceUpdateRecord,
) -> Result<(), SourceUpdateStoreError> {
    use crate::seams::source_update_store::SourceUpdateMemberOrigin;
    for member in &record.members {
        match member.origin {
            SourceUpdateMemberOrigin::Existing => {
                transaction
                    .execute(
                        "UPDATE git_source_members
                            SET presence = 'current', last_seen_release_id = ?1,
                                last_checked_at = unixepoch('now'), last_updated_at = unixepoch('now')
                          WHERE skill_id = ?2",
                        params![record.release_id, member.skill_id.0],
                    )
                    .map_err(update_sql_error)?;
                transaction
                    .execute(
                        "UPDATE skills
                            SET directory_name = ?1, directory_identity_key = ?2,
                                display_name = ?3, description = ?4, health = 'healthy',
                                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                          WHERE id = ?5",
                        params![
                            member.directory_name,
                            member.identity_key,
                            member.display_name,
                            member.description,
                            member.skill_id.0,
                        ],
                    )
                    .map_err(update_sql_error)?;
            }
            SourceUpdateMemberOrigin::New => {
                transaction
                    .execute(
                        "INSERT INTO skills (
                            id, directory_name, directory_identity_key, display_name, description,
                            source_kind, library_entry_path, final_entity_path,
                            recorded_content_hash, health, created_at, updated_at
                         ) VALUES (
                            ?1, ?2, ?3, ?4, ?5, 'remote_install', ?6, ?6, ?7, 'healthy',
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                            strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                         )",
                        params![
                            member.skill_id.0,
                            member.directory_name,
                            member.identity_key,
                            member.display_name,
                            member.description,
                            member.storage_relpath,
                            member.tree_hash,
                        ],
                    )
                    .map_err(update_sql_error)?;
                transaction
                    .execute(
                        "INSERT INTO git_source_members (
                            skill_id, remote_id, skill_path, storage_relpath, presence,
                            first_seen_release_id, last_seen_release_id, last_checked_at, last_updated_at
                         ) VALUES (?1, ?2, ?3, ?4, 'current', ?5, ?5, unixepoch('now'), unixepoch('now'))",
                        params![
                            member.skill_id.0,
                            record.remote_id,
                            member.skill_path,
                            member.storage_relpath,
                            record.release_id,
                        ],
                    )
                    .map_err(update_sql_error)?;
            }
        }
    }
    for removed in &record.removed_members {
        transaction
            .execute(
                "UPDATE git_source_members
                    SET presence = 'absent', last_seen_release_id = ?1,
                        last_checked_at = unixepoch('now'), last_updated_at = unixepoch('now')
                  WHERE skill_id = ?2",
                params![record.release_id, removed.skill_id.0],
            )
            .map_err(update_sql_error)?;
        transaction
            .execute(
                "UPDATE skills
                    SET health = 'broken',
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE id = ?1",
                [&removed.skill_id.0],
            )
            .map_err(update_sql_error)?;
    }
    Ok(())
}

fn bump_snapshot(transaction: &Transaction) -> Result<u64, SourceUpdateStoreError> {
    transaction
        .execute(
            "UPDATE catalog_meta SET snapshot_version = snapshot_version + 1 WHERE singleton = 1",
            [],
        )
        .map_err(update_sql_error)?;
    let snapshot_version: i64 = transaction
        .query_row(
            "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(update_sql_error)?;
    u64::try_from(snapshot_version)
        .map_err(|_| SourceUpdateStoreError::Unavailable("negative SQLite snapshot version".into()))
}

fn source_update_committed_with(
    connection: &Connection,
    record: &SourceUpdateRecord,
) -> Result<bool, SourceUpdateStoreError> {
    let Some(source) = read_source_row(connection, &record.remote_id)? else {
        return Ok(false);
    };
    if source.1 != record.provider
        || source.2 != record.canonical_url
        || source.3 != record.tracking_mode
        || source.4 != record.tracking_value
        || source.5 != record.selected_ref
        || source.6.as_deref() != Some(record.release_id.as_str())
    {
        return Ok(false);
    }
    let release: Option<(String, String)> = connection
        .query_row(
            "SELECT selection_kind, resolved_commit
             FROM git_source_releases WHERE release_id = ?1 AND remote_id = ?2",
            params![record.release_id, record.remote_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(update_sql_error)?;
    if release
        != Some((
            record.selection_kind.clone(),
            record.resolved_commit.clone(),
        ))
    {
        return Ok(false);
    }
    let release_members = read_release_trees(connection, &record.release_id)?;
    let mut expected_release_members = record
        .members
        .iter()
        .map(|member| (member.skill_path.clone(), member.tree_hash.clone()))
        .collect::<Vec<_>>();
    expected_release_members.sort_by(|left, right| left.0.cmp(&right.0));
    if release_members != expected_release_members {
        return Ok(false);
    }
    let live = read_live_members(connection, &record.remote_id)?;
    let mut expected_live = Vec::with_capacity(record.members.len() + record.removed_members.len());
    let previous_by_id = record
        .previous_members
        .iter()
        .map(|previous| (previous.skill_id.0.as_str(), previous))
        .collect::<std::collections::BTreeMap<_, _>>();
    for member in &record.members {
        expected_live.push((
            member.skill_id.0.clone(),
            member.directory_name.clone(),
            member.identity_key.clone(),
            member.display_name.clone(),
            member.description.clone(),
            "healthy".to_string(),
            member.skill_path.clone(),
            member.storage_relpath.clone(),
            "current".to_string(),
            record.release_id.clone(),
        ));
    }
    for removed in &record.removed_members {
        let previous = previous_by_id
            .get(removed.skill_id.0.as_str())
            .ok_or_else(|| {
                SourceUpdateStoreError::Conflict(
                    "the frozen previous member set is incomplete".into(),
                )
            })?;
        expected_live.push((
            removed.skill_id.0.clone(),
            previous.directory_name.clone(),
            previous.identity_key.clone(),
            previous.display_name.clone(),
            previous.description.clone(),
            "broken".to_string(),
            removed.skill_path.clone(),
            removed.storage_relpath.clone(),
            "absent".to_string(),
            record.release_id.clone(),
        ));
    }
    expected_live.sort_by(|left, right| {
        (left.6.as_str(), left.0.as_str()).cmp(&(right.6.as_str(), right.0.as_str()))
    });
    Ok(live == expected_live)
}

impl SourceUpdateStore for SqliteCatalogStore {
    fn read_current(
        &self,
        remote_id: &str,
    ) -> Result<Option<SourceUpdateCurrentSource>, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let Some(source) = read_source_row(&connection, remote_id)? else {
            return Ok(None);
        };
        let (
            source_remote_id,
            provider,
            canonical_url,
            tracking_mode,
            tracking_value,
            selected_ref,
            current_release_id,
            created_at,
        ) = source;
        let resolve_current_release =
            |connection: &Connection| -> Result<Option<String>, SourceUpdateStoreError> {
                match current_release_id.as_deref() {
                    None => Ok(None),
                    Some(release_id) => connection
                        .query_row(
                            "SELECT resolved_commit FROM git_source_releases
                          WHERE release_id = ?1 AND remote_id = ?2",
                            params![release_id, remote_id],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(update_sql_error),
                }
            };
        let resolved_commit = resolve_current_release(&connection)?.ok_or_else(|| {
            SourceUpdateStoreError::Conflict(
                "the current Git Repository Source release is missing".into(),
            )
        })?;
        let release_trees = match current_release_id.as_deref() {
            Some(release_id) => read_release_trees(&connection, release_id)?,
            None => Vec::new(),
        };
        let trees_by_path = release_trees
            .iter()
            .map(|(path, tree)| (path.as_str(), tree.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut members = read_live_members(&connection, remote_id)?
            .into_iter()
            .map(|member| {
                let presence = parse_presence(&member.8);
                let tree_hash = if presence == SourceMemberPresence::Current {
                    trees_by_path
                        .get(member.6.as_str())
                        .map(|tree| (*tree).to_string())
                } else {
                    None
                };
                SourceUpdateCurrentMember {
                    skill_id: SkillId(member.0),
                    directory_name: member.1,
                    identity_key: member.2,
                    display_name: member.3,
                    description: member.4,
                    skill_path: member.6,
                    storage_relpath: member.7,
                    presence,
                    tree_hash,
                    health: parse_health_row(&member.5),
                    last_seen_release_id: member.9,
                }
            })
            .collect::<Vec<_>>();
        members.sort_by(|left, right| {
            (left.skill_path.as_str(), left.skill_id.0.as_str())
                .cmp(&(right.skill_path.as_str(), right.skill_id.0.as_str()))
        });
        let aliases = connection
            .prepare(
                "SELECT alias_url FROM remote_source_aliases
                  WHERE remote_id = ?1 ORDER BY alias_url",
            )
            .map_err(update_sql_error)?
            .query_map([remote_id], |row| row.get::<_, String>(0))
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)?;
        let forbidden_local_link_roots = connection
            .prepare("SELECT configured_path FROM global_skill_roots ORDER BY path_identity_key")
            .map_err(update_sql_error)?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)?
            .into_iter()
            .map(PathBuf::from)
            .collect();
        Ok(Some(SourceUpdateCurrentSource {
            remote_id: source_remote_id,
            provider,
            canonical_url,
            aliases,
            tracking_mode,
            tracking_value,
            selected_ref,
            current_release_id: current_release_id.unwrap_or_default(),
            resolved_commit,
            created_at,
            members,
            forbidden_local_link_roots,
        }))
    }

    fn source_ids(&self) -> Result<Vec<String>, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let mut statement = connection
            .prepare("SELECT remote_id FROM git_repository_sources ORDER BY remote_id")
            .map_err(update_sql_error)?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)
    }

    fn member_health(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<(String, Health)>, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        connection
            .query_row(
                "SELECT members.remote_id, skills.health
                   FROM git_source_members members
                   JOIN skills ON skills.id = members.skill_id
                  WHERE members.skill_id = ?1",
                [&skill_id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        parse_health_row(&row.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()
            .map_err(update_sql_error)
    }

    fn validate_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<(), SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        validate_source_update_records(&connection, record)
    }

    fn commit_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(update_sql_error)?;
        validate_source_update_records(&transaction, record)?;
        transaction
            .execute(
                "INSERT INTO git_source_releases (
                    release_id, remote_id, selection_kind, selected_ref, resolved_commit, discovered_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    record.release_id,
                    record.remote_id,
                    record.selection_kind,
                    record.selected_ref,
                    record.resolved_commit,
                ],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("insert release: {error}"))
            })?;
        transaction
            .execute(
                "UPDATE git_repository_sources
                    SET tracking_mode = ?1, tracking_value = ?2,
                        current_selected_ref = ?3, current_release_id = ?4,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE remote_id = ?5",
                params![
                    record.tracking_mode,
                    record.tracking_value,
                    record.selected_ref,
                    record.release_id,
                    record.remote_id,
                ],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("update source row: {error}"))
            })?;
        commit_update_members(&transaction, record)?;
        for member in &record.members {
            transaction
                .execute(
                    "INSERT INTO git_source_release_members (
                        release_id, skill_id, skill_path, directory_name,
                        directory_identity_key, tree_hash, provider_hash
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        record.release_id,
                        member.skill_id.0,
                        member.skill_path,
                        member.directory_name,
                        member.identity_key,
                        member.tree_hash,
                        member.provider_hash,
                    ],
                )
                .map_err(|error| {
                    SourceUpdateStoreError::Unavailable(format!("insert release member: {error}"))
                })?;
        }
        let snapshot_version = bump_snapshot(&transaction)?;
        transaction.commit().map_err(update_sql_error)?;
        Ok(snapshot_version)
    }

    fn source_update_is_committed(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<bool, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        source_update_committed_with(&connection, record)
    }

    fn undo_source_update(
        &self,
        record: &SourceUpdateRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(update_sql_error)?;
        if !source_update_committed_with(&transaction, record)? {
            return Err(SourceUpdateStoreError::Conflict(
                "the Source Update is no longer the current release; Undo is refused".into(),
            ));
        }
        use crate::seams::source_update_store::SourceUpdateMemberOrigin;
        let new_skill_ids = record
            .members
            .iter()
            .filter(|member| member.origin == SourceUpdateMemberOrigin::New)
            .map(|member| member.skill_id.0.clone())
            .collect::<Vec<_>>();
        for member in &record.members {
            if member.origin == SourceUpdateMemberOrigin::New {
                transaction
                    .execute(
                        "DELETE FROM git_source_members WHERE skill_id = ?1",
                        [&member.skill_id.0],
                    )
                    .map_err(|error| {
                        SourceUpdateStoreError::Unavailable(format!(
                            "undo detach new member: {error}"
                        ))
                    })?;
            }
        }
        // Detach every member row from the target release first: member
        // `last_seen_release_id` references the release without CASCADE.
        transaction
            .execute(
                "UPDATE git_source_members
                    SET last_seen_release_id = ?1
                  WHERE remote_id = ?2",
                params![record.previous_release_id, record.remote_id],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("undo detach: {error}"))
            })?;
        transaction
            .execute(
                "UPDATE git_repository_sources
                    SET tracking_mode = ?1, tracking_value = ?2,
                        current_selected_ref = ?3, current_release_id = ?4,
                        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                  WHERE remote_id = ?5",
                params![
                    record.previous_tracking_mode,
                    record.previous_tracking_value,
                    record.previous_selected_ref,
                    record.previous_release_id,
                    record.remote_id,
                ],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("undo source row: {error}"))
            })?;
        transaction
            .execute(
                "DELETE FROM git_source_releases
                  WHERE release_id = ?1 AND remote_id = ?2",
                params![record.release_id, record.remote_id],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("undo delete release: {error}"))
            })?;
        for skill_id in &new_skill_ids {
            transaction
                .execute("DELETE FROM skills WHERE id = ?1", [skill_id])
                .map_err(|error| {
                    SourceUpdateStoreError::Unavailable(format!("undo delete new: {error}"))
                })?;
        }
        for member in &record.members {
            match member.origin {
                SourceUpdateMemberOrigin::New => {}
                SourceUpdateMemberOrigin::Existing => {
                    let previous = record
                        .previous_members
                        .iter()
                        .find(|previous| previous.skill_id.0 == member.skill_id.0)
                        .ok_or_else(|| {
                            SourceUpdateStoreError::Conflict(
                                "the frozen previous member set is incomplete".into(),
                            )
                        })?;
                    transaction
                        .execute(
                            "UPDATE git_source_members
                                SET presence = ?1, last_seen_release_id = ?2,
                                    last_checked_at = unixepoch('now'), last_updated_at = unixepoch('now')
                              WHERE skill_id = ?3",
                            params![
                                presence_value(previous.presence),
                                record.previous_release_id,
                                member.skill_id.0,
                            ],
                        )
                        .map_err(update_sql_error)?;
                    transaction
                        .execute(
                            "UPDATE skills
                                SET directory_name = ?1, directory_identity_key = ?2,
                                    display_name = ?3, description = ?4, health = ?5,
                                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                              WHERE id = ?6",
                            params![
                                previous.directory_name,
                                previous.identity_key,
                                previous.display_name,
                                previous.description,
                                health_text(previous.health),
                                member.skill_id.0,
                            ],
                        )
                        .map_err(update_sql_error)?;
                }
            }
        }
        for removed in &record.removed_members {
            let previous = record
                .previous_members
                .iter()
                .find(|previous| previous.skill_id.0 == removed.skill_id.0)
                .ok_or_else(|| {
                    SourceUpdateStoreError::Conflict(
                        "the frozen previous member set is incomplete".into(),
                    )
                })?;
            transaction
                .execute(
                    "UPDATE git_source_members
                        SET presence = ?1, last_seen_release_id = ?2,
                            last_checked_at = unixepoch('now'), last_updated_at = unixepoch('now')
                      WHERE skill_id = ?3",
                    params![
                        presence_value(previous.presence),
                        record.previous_release_id,
                        removed.skill_id.0,
                    ],
                )
                .map_err(update_sql_error)?;
            transaction
                .execute(
                    "UPDATE skills
                        SET directory_name = ?1, directory_identity_key = ?2,
                            display_name = ?3, description = ?4, health = ?5,
                            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                      WHERE id = ?6",
                    params![
                        previous.directory_name,
                        previous.identity_key,
                        previous.display_name,
                        previous.description,
                        health_text(previous.health),
                        removed.skill_id.0,
                    ],
                )
                .map_err(update_sql_error)?;
        }
        let snapshot_version = bump_snapshot(&transaction)?;
        transaction.commit().map_err(update_sql_error)?;
        Ok(snapshot_version)
    }

    fn set_source_member_health(
        &self,
        remote_id: &str,
        health: &[(SkillId, Health)],
    ) -> Result<u64, SourceUpdateStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(update_sql_error)?;
        for (skill_id, member_health) in health {
            let member: Option<()> = transaction
                .query_row(
                    "SELECT 1 FROM git_source_members
                      WHERE skill_id = ?1 AND remote_id = ?2",
                    params![skill_id.0, remote_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(update_sql_error)?;
            if member.is_none() {
                return Err(SourceUpdateStoreError::Conflict(
                    "a member to update no longer belongs to this source".into(),
                ));
            }
            transaction
                .execute(
                    "UPDATE skills
                        SET health = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                      WHERE id = ?2",
                    params![health_text(*member_health), skill_id.0],
                )
                .map_err(update_sql_error)?;
        }
        let snapshot_version = bump_snapshot(&transaction)?;
        transaction.commit().map_err(update_sql_error)?;
        Ok(snapshot_version)
    }

    fn register_local_copy(
        &self,
        record: &LocalSourceCopyRecord,
    ) -> Result<u64, SourceUpdateStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(update_sql_error)?;
        let conflict: Option<String> = transaction
            .query_row(
                "SELECT directory_name FROM skills
                  WHERE directory_identity_key = ?1
                    AND NOT EXISTS (
                        SELECT 1 FROM git_source_members members
                         WHERE members.skill_id = skills.id
                    )
                  LIMIT 1",
                [&record.identity_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(update_sql_error)?;
        if let Some(directory_name) = conflict {
            return Err(SourceUpdateStoreError::Conflict(format!(
                "the Library already contains Managed Skill '{directory_name}'"
            )));
        }
        transaction
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path, health,
                    created_at, updated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, 'link', NULL, ?6, 'healthy',
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 )",
                params![
                    record.skill_id.0,
                    record.directory_name,
                    record.identity_key,
                    record.display_name,
                    record.description,
                    record.final_entity_path.to_string_lossy(),
                ],
            )
            .map_err(update_sql_error)?;
        let snapshot_version = bump_snapshot(&transaction)?;
        transaction.commit().map_err(update_sql_error)?;
        Ok(snapshot_version)
    }

    fn local_copy_is_registered(&self, destination: &Path) -> Result<bool, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let path_text = destination.to_string_lossy().into_owned();
        connection
            .query_row(
                "SELECT 1 FROM skills
                  WHERE source_kind = 'link' AND final_entity_path = ?1
                  LIMIT 1",
                [&path_text],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(update_sql_error)
    }

    fn source_remove_facts(
        &self,
        remote_id: &str,
    ) -> Result<SourceRemoveFacts, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let source = read_source_row(&connection, remote_id)?.ok_or_else(|| {
            SourceUpdateStoreError::Conflict(
                "the Git Repository Source is no longer complete".into(),
            )
        })?;
        let member_skill_ids = connection
            .prepare(
                "SELECT skill_id FROM git_source_members
                  WHERE remote_id = ?1 ORDER BY skill_id",
            )
            .map_err(update_sql_error)?
            .query_map([remote_id], |row| row.get::<_, String>(0))
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)?
            .into_iter()
            .map(SkillId)
            .collect::<Vec<_>>();
        let activations = connection
            .prepare(
                "SELECT activations.skill_id, activations.expected_entry_path,
                        activations.expected_target_path
                   FROM activations
                   JOIN git_source_members members ON members.skill_id = activations.skill_id
                  WHERE members.remote_id = ?1 AND activations.desired_enabled = 1
                  ORDER BY activations.expected_entry_path, activations.skill_id",
            )
            .map_err(update_sql_error)?
            .query_map([remote_id], |row| {
                Ok(
                    crate::seams::source_update_store::SourceRemoveActivationFacts {
                        skill_id: SkillId(row.get(0)?),
                        entry_path: PathBuf::from(row.get::<_, String>(1)?),
                        target_path: PathBuf::from(row.get::<_, String>(2)?),
                    },
                )
            })
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)?;
        Ok(SourceRemoveFacts {
            remote_id: remote_id.into(),
            canonical_url: source.2,
            member_skill_ids,
            activations,
        })
    }

    fn commit_remove_source(&self, remote_id: &str) -> Result<u64, SourceUpdateStoreError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(update_sql_error)?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM git_repository_sources WHERE remote_id = ?1
                )",
                [remote_id],
                |row| row.get(0),
            )
            .map_err(update_sql_error)?;
        if !exists {
            // Idempotent recovery probe: a Remove that already committed is
            // a removed source.
            let snapshot_version: i64 = transaction
                .query_row(
                    "SELECT snapshot_version FROM catalog_meta WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(update_sql_error)?;
            transaction.commit().map_err(update_sql_error)?;
            return u64::try_from(snapshot_version).map_err(|_| {
                SourceUpdateStoreError::Unavailable("negative SQLite snapshot version".into())
            });
        }
        let member_skill_ids = transaction
            .prepare(
                "SELECT skill_id FROM git_source_members
                  WHERE remote_id = ?1 ORDER BY skill_id",
            )
            .map_err(update_sql_error)?
            .query_map([remote_id], |row| row.get::<_, String>(0))
            .map_err(update_sql_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(update_sql_error)?;
        transaction
            .execute(
                "DELETE FROM remote_source_parents WHERE remote_id = ?1",
                [remote_id],
            )
            .map_err(|error| {
                SourceUpdateStoreError::Unavailable(format!("delete source parent: {error}"))
            })?;
        for skill_id in &member_skill_ids {
            transaction
                .execute("DELETE FROM skills WHERE id = ?1", [skill_id])
                .map_err(|error| {
                    SourceUpdateStoreError::Unavailable(format!("delete source skill: {error}"))
                })?;
        }
        let snapshot_version = bump_snapshot(&transaction)?;
        transaction.commit().map_err(update_sql_error)?;
        Ok(snapshot_version)
    }

    fn source_remove_is_committed(&self, remote_id: &str) -> Result<bool, SourceUpdateStoreError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SourceUpdateStoreError::Unavailable("SQLite lock poisoned".into()))?;
        connection
            .query_row(
                "SELECT NOT EXISTS(
                    SELECT 1 FROM git_repository_sources WHERE remote_id = ?1
                )",
                [remote_id],
                |row| row.get(0),
            )
            .map_err(update_sql_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_round_trips_closed_values() {
        assert_eq!(presence_value(SourceMemberPresence::Current), "current");
        assert_eq!(presence_value(SourceMemberPresence::Absent), "absent");
        assert_eq!(parse_presence("current"), SourceMemberPresence::Current);
        assert_eq!(parse_presence("absent"), SourceMemberPresence::Absent);
    }

    #[test]
    fn health_round_trips_closed_values() {
        assert_eq!(
            health_text(Health::SourceSnapshotMismatch),
            "source_snapshot_mismatch"
        );
        assert_eq!(
            parse_health_row("source_snapshot_mismatch"),
            Health::SourceSnapshotMismatch
        );
        assert_eq!(parse_health_row("healthy"), Health::Healthy);
    }
}
