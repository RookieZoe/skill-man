//! Scan Run root planning (spec §4.10, §10.1 "九个 Preset 与 Custom Agent
//! 多 Root"): the canonical union of the configured Global Skills Roots —
//! one physical Root scanned once, deduplicated by directory identity and
//! frozen as `(configured, canonical, device, inode)`. A configured Root
//! that cannot be canonicalized is still part of the union plan with a
//! typed plan error: its coverage row fails (Never silently dropped), while
//! the rest of the Run continues.

use std::path::PathBuf;

use crate::seams::agent_configuration_store::{
    AgentConfigurationStoreSnapshot, StoredGlobalSkillRoot,
};
use crate::seams::filesystem::{DirectoryFingerprint, FileSystem};
use crate::seams::scan_evidence_store::ScanFrozenRoot;
use crate::seams::scan_integrity::canonical_json_digest;

/// One planned Root: either frozen (canonical identity resolved) or
/// failed-at-plan with a typed diagnostic (unreadable, identity-less, …).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedRoot {
    pub index: u32,
    pub configured_path: PathBuf,
    pub frozen: Option<ScanFrozenRoot>,
    pub plan_error: Option<String>,
}

/// The frozen identity fingerprint of the whole configured Root snapshot:
/// any Root set or identity change makes a frozen Run superseded.
pub fn roots_fingerprint(roots: &[ScanFrozenRoot]) -> String {
    let mut pairs: Vec<(String, String, u64, u64)> = roots
        .iter()
        .map(|root| {
            (
                root.configured_path.to_string_lossy().into_owned(),
                root.canonical_path.to_string_lossy().into_owned(),
                root.device,
                root.inode,
            )
        })
        .collect();
    pairs.sort();
    let value = serde_json::to_value(&pairs).expect("root pair list serializes");
    format!("roots<{}>{}", pairs.len(), canonical_json_digest(&value))
}

/// The full planned set: successful canonical roots plus typed plan
/// failures, so the engine can publish failed Root coverage rows instead of
/// silently dropping unreadable configured Roots.
pub fn plan_roots_with_failures(
    filesystem: &dyn FileSystem,
    snapshot: &AgentConfigurationStoreSnapshot,
) -> Result<Vec<PlannedRoot>, PlanError> {
    let mut planned = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, root) in snapshot.roots.iter().enumerate() {
        let configured = root.configured_path.clone();
        match canonical_identity(filesystem, root, index as u32) {
            Ok(identity)
                if seen.insert((
                    identity.canonical_path.clone(),
                    identity.device,
                    identity.inode,
                )) =>
            {
                planned.push(PlannedRoot {
                    index: index as u32,
                    configured_path: configured,
                    frozen: Some(identity),
                    plan_error: None,
                });
            }
            // The same physical Root reached through a second configured
            // path is scanned once (spec §10.1 "同一物理 Root 一次").
            Ok(_) => {}
            Err(PlanError::Canonicalize(error)) => planned.push(PlannedRoot {
                index: index as u32,
                configured_path: configured,
                frozen: None,
                plan_error: Some(error),
            }),
        }
    }
    Ok(planned)
}

fn canonical_identity(
    filesystem: &dyn FileSystem,
    root: &StoredGlobalSkillRoot,
    index: u32,
) -> Result<ScanFrozenRoot, PlanError> {
    let canonical = filesystem
        .canonical_directory(&root.configured_path)
        .map_err(|error| {
            PlanError::Canonicalize(format!("{}: {error}", root.configured_path.display()))
        })?;
    let fingerprint = filesystem
        .directory_fingerprint(&canonical)
        .map_err(|error| PlanError::Canonicalize(format!("{canonical:?}: {error}")))?;
    let DirectoryFingerprint {
        canonical_path,
        device,
        inode,
    } = fingerprint;
    Ok(ScanFrozenRoot {
        index,
        configured_path: root.configured_path.clone(),
        canonical_path,
        device,
        inode,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("a configured Root could not be canonicalized: {0}")]
    Canonicalize(String),
}

/// Test composition: planned roots with a failure-free union.
#[cfg(test)]
pub fn planned_counts(
    planned: &[PlannedRoot],
) -> crate::seams::scan_evidence_store::ScanEvidenceCounts {
    crate::seams::scan_evidence_store::ScanEvidenceCounts {
        roots: planned.len() as u64,
        failed_roots: planned
            .iter()
            .filter(|planned| planned.frozen.is_none())
            .count() as u64,
        ..Default::default()
    }
}
