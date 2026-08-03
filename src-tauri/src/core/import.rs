use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::{SkillId, parse_skill_metadata, skill_identity_key};
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    FileSystem, FileSystemError, LinkSourceEntryKind, LinkSourceSnapshot, SkillFingerprint,
};
use crate::seams::import_store::{
    ImportStore, ImportStoreError, LibraryConflict as StoreLibraryConflict, LinkImportRecord,
};

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkImportCandidate {
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub source_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryConflict {
    pub existing_skill_id: SkillId,
    pub directory_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkImportPreview {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub source_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub conflict: Option<LibraryConflict>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkImportResult {
    pub operation_id: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
    pub snapshot_version: u64,
}

#[derive(Clone)]
struct PlannedLinkImport {
    candidate: LinkImportCandidate,
    source_snapshot: LinkSourceSnapshot,
    skill_id: SkillId,
    fingerprint: SkillFingerprint,
    conflict: Option<LibraryConflict>,
    created_at_millis: u128,
}

pub struct ImportService {
    store: Arc<dyn ImportStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    library_root: PathBuf,
    plans: Mutex<HashMap<String, PlannedLinkImport>>,
    next_plan_id: AtomicU64,
    next_skill_id: AtomicU64,
    plan_ttl: Duration,
}

impl ImportService {
    pub fn new(
        store: Arc<dyn ImportStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
    ) -> Self {
        Self {
            store,
            filesystem,
            clock,
            library_root,
            plans: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            next_skill_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
        }
    }

    pub fn discover_link(&self, source_path: &Path) -> Result<LinkImportCandidate, ImportError> {
        self.discover_link_with_snapshot(source_path)
            .map(|(candidate, _)| candidate)
    }

    fn discover_link_with_snapshot(
        &self,
        source_path: &Path,
    ) -> Result<(LinkImportCandidate, LinkSourceSnapshot), ImportError> {
        let source_snapshot = self.filesystem.inspect_link_source(source_path)?;
        validate_utf8_link_source(&source_snapshot)?;
        let final_entity_path = source_snapshot.final_entity_path.clone();
        let library_root = self
            .filesystem
            .normalize_configured_path(&self.library_root)?;
        if source_snapshot.entry_path.starts_with(&library_root)
            || final_entity_path.starts_with(&library_root)
        {
            return Err(ImportError::Validation(
                "Link sources must remain outside the Library".into(),
            ));
        }
        let directory_name = source_snapshot.directory_name.clone();
        let identity_key = normalize_identity(&directory_name)?;
        if !self
            .filesystem
            .skill_directory_is_readable(&final_entity_path)?
        {
            return Err(ImportError::SourceUnavailable(final_entity_path));
        }
        let skill_markdown = self.filesystem.read_skill_document(&final_entity_path)?;
        let metadata = parse_skill_metadata(&skill_markdown);
        Ok((
            LinkImportCandidate {
                directory_name: directory_name.clone(),
                identity_key,
                display_name: metadata.name.clone().unwrap_or(directory_name),
                description: metadata.description.unwrap_or_default(),
                frontmatter_name: metadata.name,
                source_entry_path: source_snapshot.entry_path.clone(),
                final_entity_path,
            },
            source_snapshot,
        ))
    }

    pub fn plan_link(&self, source_path: &Path) -> Result<LinkImportPreview, ImportError> {
        let (candidate, source_snapshot) = self.discover_link_with_snapshot(source_path)?;
        let fingerprint = self
            .filesystem
            .skill_fingerprint(&candidate.final_entity_path)?;
        let conflict = self
            .store
            .find_library_conflict(&candidate.identity_key)?
            .map(map_library_conflict);
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("link-import-plan-{plan_number}");
        let skill_id = SkillId(format!(
            "link-{}-{}",
            self.clock.unix_epoch_nanos(),
            self.next_skill_id.fetch_add(1, Ordering::Relaxed)
        ));
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ImportError::Internal("Import plan lock poisoned".into()))?;
        let now = self.clock.monotonic_millis();
        plans.retain(|_, plan| {
            now.saturating_sub(plan.created_at_millis) < self.plan_ttl.as_millis()
        });
        plans.insert(
            plan_token.clone(),
            PlannedLinkImport {
                candidate: candidate.clone(),
                source_snapshot,
                skill_id,
                fingerprint,
                conflict: conflict.clone(),
                created_at_millis: now,
            },
        );
        Ok(LinkImportPreview {
            plan_token,
            directory_name: candidate.directory_name,
            display_name: candidate.display_name,
            description: candidate.description,
            source_entry_path: candidate.source_entry_path,
            final_entity_path: candidate.final_entity_path,
            can_apply: conflict.is_none(),
            conflict,
        })
    }

    pub fn apply_link(&self, plan_token: &str) -> Result<LinkImportResult, ImportError> {
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ImportError::Internal("Import plan lock poisoned".into()))?;
        let now = self.clock.monotonic_millis();
        plans.retain(|_, plan| {
            now.saturating_sub(plan.created_at_millis) < self.plan_ttl.as_millis()
        });
        let plan = plans.remove(plan_token).ok_or(ImportError::PlanNotFound)?;
        drop(plans);
        if let Some(conflict) = plan.conflict {
            return Err(ImportError::Conflict(conflict.directory_name));
        }

        let current_source = self
            .filesystem
            .inspect_link_source(&plan.source_snapshot.entry_path)
            .map_err(|_| ImportError::PlanStale)?;
        if current_source != plan.source_snapshot {
            return Err(ImportError::PlanStale);
        }
        let current = self
            .discover_link(&plan.source_snapshot.entry_path)
            .map_err(|_| ImportError::PlanStale)?;
        let current_fingerprint = self
            .filesystem
            .skill_fingerprint(&current.final_entity_path)
            .map_err(|_| ImportError::PlanStale)?;
        if current_fingerprint != plan.fingerprint
            || current.directory_name != plan.candidate.directory_name
            || current.identity_key != plan.candidate.identity_key
        {
            return Err(ImportError::PlanStale);
        }
        if let Some(conflict) = self.store.find_library_conflict(&current.identity_key)? {
            return Err(ImportError::Conflict(conflict.directory_name));
        }

        let snapshot_version = self.store.insert_link(LinkImportRecord {
            skill_id: plan.skill_id.clone(),
            directory_name: current.directory_name.clone(),
            identity_key: current.identity_key,
            display_name: current.display_name,
            description: current.description,
            final_entity_path: current.final_entity_path.clone(),
        })?;
        Ok(LinkImportResult {
            operation_id: format!("link-import-{}", plan.skill_id.0),
            skill_id: plan.skill_id,
            directory_name: current.directory_name,
            final_entity_path: current.final_entity_path,
            snapshot_version,
        })
    }

    pub fn cancel_link(&self, plan_token: &str) -> Result<bool, ImportError> {
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ImportError::Internal("Import plan lock poisoned".into()))?;
        let now = self.clock.monotonic_millis();
        plans.retain(|_, plan| {
            now.saturating_sub(plan.created_at_millis) < self.plan_ttl.as_millis()
        });
        Ok(plans.remove(plan_token).is_some())
    }
}

fn validate_utf8_link_source(snapshot: &LinkSourceSnapshot) -> Result<(), ImportError> {
    let entry_target_is_utf8 = match &snapshot.entry_kind {
        LinkSourceEntryKind::Directory => true,
        LinkSourceEntryKind::Symlink { target } => target.to_str().is_some(),
    };
    let chain_is_utf8 = snapshot
        .symlink_chain
        .iter()
        .all(|hop| hop.path.to_str().is_some() && hop.target.to_str().is_some());
    if snapshot.entry_path.to_str().is_none()
        || snapshot.final_entity_path.to_str().is_none()
        || !entry_target_is_utf8
        || !chain_is_utf8
    {
        return Err(ImportError::Validation(
            "Link source paths must be valid UTF-8".into(),
        ));
    }
    Ok(())
}

fn normalize_identity(directory_name: &str) -> Result<String, ImportError> {
    if directory_name.is_empty()
        || directory_name == "."
        || directory_name == ".."
        || directory_name.starts_with('.')
        || directory_name.trim() != directory_name
        || directory_name.len() > 128
        || directory_name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\0'))
    {
        return Err(ImportError::Validation(format!(
            "'{directory_name}' is not a valid Skill directory identity"
        )));
    }
    Ok(skill_identity_key(directory_name))
}

fn map_library_conflict(conflict: StoreLibraryConflict) -> LibraryConflict {
    LibraryConflict {
        existing_skill_id: conflict.skill_id,
        directory_name: conflict.directory_name,
    }
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("{0}")]
    Validation(String),
    #[error("the Library already contains Managed Skill '{0}'")]
    Conflict(String),
    #[error("the Link source is unavailable: {}", .0.display())]
    SourceUnavailable(PathBuf),
    #[error("the Import preview is stale; refresh it before applying")]
    PlanStale,
    #[error("the Import preview was not found or expired")]
    PlanNotFound,
    #[error(transparent)]
    Store(#[from] ImportStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error("{0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::{ImportError, validate_utf8_link_source};
    use crate::seams::filesystem::{LinkSourceEntryKind, LinkSourceHop, LinkSourceSnapshot};

    #[test]
    fn non_utf8_link_paths_are_rejected_before_preview() {
        let invalid_target =
            PathBuf::from("/tmp").join(OsString::from_vec(vec![b't', b'a', b'r', b'g', 0xff]));
        let source_path = PathBuf::from("/tmp/selected-skill");
        let snapshot = LinkSourceSnapshot {
            entry_path: source_path.clone(),
            directory_name: "selected-skill".into(),
            entry_device: 1,
            entry_inode: 2,
            entry_kind: LinkSourceEntryKind::Symlink {
                target: invalid_target.clone(),
            },
            symlink_chain: vec![LinkSourceHop {
                path: source_path,
                target: invalid_target.clone(),
                device: 1,
                inode: 2,
            }],
            final_entity_path: invalid_target,
        };

        let error = validate_utf8_link_source(&snapshot).expect_err("non-UTF-8 paths are rejected");
        assert!(matches!(error, ImportError::Validation(_)));
        assert!(error.to_string().contains("valid UTF-8"));
    }
}
