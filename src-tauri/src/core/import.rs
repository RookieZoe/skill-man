use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::{SkillId, parse_skill_metadata, skill_identity_key};
use crate::seams::activation_store::DesiredActivation;
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    DirectoryFingerprint, FileImportJournal, FileImportJournalItem, FileImportJournalPhase,
    FileReplacement, FileSystem, FileSystemError, LinkSourceEntryKind, LinkSourceSnapshot,
    SkillFingerprint, StagedEntryKind, StagedTreeSnapshot,
};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict as StoreLibraryConflict,
    LinkImportRecord,
};
use crate::seams::recovery::RecoveryGate;
use crate::seams::source::{FileSource, SourceError, StagedFileSource};

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_SKILL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SKILL_DOCUMENT_BYTES: u64 = 512 * 1024;
const MAX_SOURCE_SKILLS: usize = 100;
const DISK_SPACE_RESERVE_BYTES: u64 = 100 * 1024 * 1024;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportCandidate {
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub original_path: PathBuf,
    pub original_filename: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportPreview {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub original_path: PathBuf,
    pub original_filename: String,
    pub final_entity_path: PathBuf,
    pub conflict: Option<LibraryConflict>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportResult {
    pub operation_id: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
    pub snapshot_version: u64,
    pub reinstalled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportDiscovery {
    pub candidates: Vec<FileImportCandidate>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportSelectionPreview {
    pub plan_token: String,
    pub items: Vec<FileImportPreview>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportSelectionResult {
    pub operation_id: String,
    pub items: Vec<FileImportResult>,
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

#[derive(Clone)]
struct PlannedFileImport {
    candidate: FileImportCandidate,
    staged_source: StagedFileSource,
    staging_operation_root: PathBuf,
    staging_fingerprint: DirectoryFingerprint,
    final_entity_path: PathBuf,
    skill_id: SkillId,
    tree_snapshot: StagedTreeSnapshot,
    conflict: Option<LibraryConflict>,
    operation_id: String,
    created_at_millis: u128,
    reinstall: Option<PlannedFileReinstall>,
}

#[derive(Clone)]
struct PlannedFileReinstall {
    existing_record: FileImportRecord,
    existing_tree_snapshot: StagedTreeSnapshot,
    activations: Vec<DesiredActivation>,
}

enum AppliedFileChange {
    New(DirectoryFingerprint),
    Replacement(FileReplacement),
}

type ValidatedFileCandidate = (FileImportCandidate, StagedFileSource, StagedTreeSnapshot);
type ValidatedFileSource = (Vec<ValidatedFileCandidate>, DirectoryFingerprint, bool);

#[derive(Clone)]
struct PlannedFileImportBatch {
    items: Vec<PlannedFileImport>,
    staging_operation_root: PathBuf,
    staging_fingerprint: DirectoryFingerprint,
    operation_id: String,
    created_at_millis: u128,
}

pub struct ImportService {
    store: Arc<dyn ImportStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    file_source: Arc<dyn FileSource>,
    library_root: PathBuf,
    plans: Mutex<HashMap<String, PlannedLinkImport>>,
    file_plans: Mutex<HashMap<String, PlannedFileImport>>,
    file_batch_plans: Mutex<HashMap<String, PlannedFileImportBatch>>,
    next_plan_id: AtomicU64,
    next_skill_id: AtomicU64,
    plan_ttl: Duration,
    recovery_gate: Arc<RecoveryGate>,
}

impl ImportService {
    pub fn new(
        store: Arc<dyn ImportStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        file_source: Arc<dyn FileSource>,
        library_root: PathBuf,
    ) -> Self {
        Self {
            store,
            filesystem,
            clock,
            file_source,
            library_root,
            plans: Mutex::new(HashMap::new()),
            file_plans: Mutex::new(HashMap::new()),
            file_batch_plans: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            next_skill_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
            recovery_gate: Arc::new(RecoveryGate::ready()),
        }
    }

    pub fn with_recovery_gate(mut self, recovery_gate: Arc<RecoveryGate>) -> Self {
        self.recovery_gate = recovery_gate;
        self
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
        self.ensure_writes_ready()?;
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

    pub fn discover_file(&self, source_path: &Path) -> Result<FileImportCandidate, ImportError> {
        self.ensure_writes_ready()?;
        let operation_id = self.next_file_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let (mut candidates, staging_fingerprint, _) =
            self.stage_and_validate_file(source_path, &staging_operation_root)?;
        let result = if candidates.len() == 1 {
            Ok(candidates.remove(0).0)
        } else {
            Err(ImportError::Validation(format!(
                "file Import discovered {} Skills; use candidate selection",
                candidates.len()
            )))
        };
        let cleanup = self.filesystem.discard_staging(
            &staging_operation_root,
            &self.library_root,
            Some(&staging_fingerprint),
        );
        match (result, cleanup) {
            (Ok(candidate), Ok(())) => Ok(candidate),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    pub fn discover_file_collection(
        &self,
        source_path: &Path,
    ) -> Result<FileImportDiscovery, ImportError> {
        self.ensure_writes_ready()?;
        let operation_id = self.next_file_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let (staged, staging_fingerprint, truncated) =
            self.stage_and_validate_file(source_path, &staging_operation_root)?;
        let discovery = FileImportDiscovery {
            candidates: staged
                .into_iter()
                .map(|(candidate, _, _)| candidate)
                .collect(),
            truncated,
        };
        match self.filesystem.discard_staging(
            &staging_operation_root,
            &self.library_root,
            Some(&staging_fingerprint),
        ) {
            Ok(()) => Ok(discovery),
            Err(error) => Err(ImportError::RecoveryRequired(format!(
                "candidate discovery succeeded, but staging cleanup failed: {error}"
            ))),
        }
    }

    pub fn plan_file_selection(
        &self,
        source_path: &Path,
        selected_directory_names: &[String],
    ) -> Result<FileImportSelectionPreview, ImportError> {
        self.ensure_writes_ready()?;
        let operation_id = self.next_file_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let selected = selected_directory_names
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let selection_filter = (!selected.is_empty()).then_some(&selected);
        let (staged, staging_fingerprint, _) = self.stage_and_validate_file_selection(
            source_path,
            &staging_operation_root,
            selection_filter,
        )?;
        let selected_count = selected.len();
        let chosen = staged
            .into_iter()
            .filter(|(candidate, _, _)| {
                selected.is_empty() || selected.contains(&candidate.directory_name)
            })
            .collect::<Vec<_>>();
        if chosen.is_empty() || (!selected.is_empty() && chosen.len() != selected_count) {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::Validation(
                    "file Import selection contains an unknown or duplicate Skill name".into(),
                ),
            );
        }
        let required_space = chosen
            .iter()
            .map(|(_, _, snapshot)| snapshot.total_file_bytes)
            .fold(0_u64, u64::saturating_add)
            .saturating_mul(2)
            .saturating_add(DISK_SPACE_RESERVE_BYTES);
        let available_space = match self.filesystem.available_space(&self.library_root) {
            Ok(space) => space,
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        if available_space < required_space {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::DiskFull {
                    required_bytes: required_space,
                    available_bytes: available_space,
                },
            );
        }

        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("file-import-selection-plan-{plan_number}");
        let created_at_millis = self.clock.monotonic_millis();
        let mut items = Vec::with_capacity(chosen.len());
        let mut previews = Vec::with_capacity(chosen.len());
        for (candidate, staged_source, tree_snapshot) in chosen {
            let conflict = match self.store.find_library_conflict(&candidate.identity_key) {
                Ok(conflict) => conflict.map(map_library_conflict),
                Err(error) => {
                    return self.cleanup_staging_after_error(
                        &staging_operation_root,
                        Some(&staging_fingerprint),
                        error.into(),
                    );
                }
            };
            let final_entity_path = self
                .library_root
                .join("skills")
                .join(&candidate.directory_name);
            let skill_id = SkillId(format!(
                "file-{}-{}",
                self.clock.unix_epoch_nanos(),
                self.next_skill_id.fetch_add(1, Ordering::Relaxed)
            ));
            items.push(PlannedFileImport {
                candidate: candidate.clone(),
                staged_source,
                staging_operation_root: staging_operation_root.clone(),
                staging_fingerprint: staging_fingerprint.clone(),
                final_entity_path: final_entity_path.clone(),
                skill_id,
                tree_snapshot,
                conflict: conflict.clone(),
                operation_id: operation_id.clone(),
                created_at_millis,
                reinstall: None,
            });
            previews.push(FileImportPreview {
                plan_token: plan_token.clone(),
                directory_name: candidate.directory_name,
                display_name: candidate.display_name,
                description: candidate.description,
                original_path: candidate.original_path,
                original_filename: candidate.original_filename,
                final_entity_path,
                can_apply: conflict.is_none(),
                conflict,
            });
        }
        let can_apply = previews.iter().all(|preview| preview.can_apply);
        let batch = PlannedFileImportBatch {
            items,
            staging_operation_root,
            staging_fingerprint,
            operation_id,
            created_at_millis,
        };
        let journal = file_import_journal(
            &batch.operation_id,
            &batch.staging_operation_root,
            &batch.staging_fingerprint,
            &batch.items,
        );
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return self.abort_unapplied_file_journal(
                &batch.operation_id,
                &batch.staging_operation_root,
                &batch.staging_fingerprint,
                error.into(),
            );
        }
        let mut batch_plans = match self.file_batch_plans.lock() {
            Ok(plans) => plans,
            Err(_) => {
                return self.abort_unapplied_file_journal(
                    &batch.operation_id,
                    &batch.staging_operation_root,
                    &batch.staging_fingerprint,
                    ImportError::Internal("file Import batch plan lock poisoned".into()),
                );
            }
        };
        batch_plans.insert(plan_token.clone(), batch);
        Ok(FileImportSelectionPreview {
            plan_token,
            items: previews,
            can_apply,
        })
    }

    pub fn apply_file_selection(
        &self,
        plan_token: &str,
    ) -> Result<FileImportSelectionResult, ImportError> {
        self.ensure_writes_ready()?;
        let batch = self
            .file_batch_plans
            .lock()
            .map_err(|_| ImportError::Internal("file Import batch plan lock poisoned".into()))?
            .remove(plan_token)
            .ok_or(ImportError::PlanNotFound)?;
        if self
            .clock
            .monotonic_millis()
            .saturating_sub(batch.created_at_millis)
            >= self.plan_ttl.as_millis()
        {
            return self.abort_unapplied_file_journal(
                &batch.operation_id,
                &batch.staging_operation_root,
                &batch.staging_fingerprint,
                ImportError::PlanNotFound,
            );
        }
        for item in &batch.items {
            if let Some(conflict) = &item.conflict {
                return self.abort_unapplied_file_journal(
                    &batch.operation_id,
                    &batch.staging_operation_root,
                    &batch.staging_fingerprint,
                    ImportError::Conflict(conflict.directory_name.clone()),
                );
            }
            let current_conflict = match self
                .store
                .find_library_conflict(&item.candidate.identity_key)
            {
                Ok(conflict) => conflict,
                Err(error) => {
                    return self.abort_unapplied_file_journal(
                        &batch.operation_id,
                        &batch.staging_operation_root,
                        &batch.staging_fingerprint,
                        error.into(),
                    );
                }
            };
            if let Some(conflict) = current_conflict {
                return self.abort_unapplied_file_journal(
                    &batch.operation_id,
                    &batch.staging_operation_root,
                    &batch.staging_fingerprint,
                    ImportError::Conflict(conflict.directory_name),
                );
            }
            let current = match self
                .filesystem
                .staged_tree_snapshot(&item.staged_source.staged_content_root)
            {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    return self.abort_unapplied_file_journal(
                        &batch.operation_id,
                        &batch.staging_operation_root,
                        &batch.staging_fingerprint,
                        ImportError::PlanStale,
                    );
                }
            };
            if current != item.tree_snapshot {
                return self.abort_unapplied_file_journal(
                    &batch.operation_id,
                    &batch.staging_operation_root,
                    &batch.staging_fingerprint,
                    ImportError::PlanStale,
                );
            }
        }

        let mut journal = file_import_journal(
            &batch.operation_id,
            &batch.staging_operation_root,
            &batch.staging_fingerprint,
            &batch.items,
        );
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return self.abort_unapplied_file_journal(
                &batch.operation_id,
                &batch.staging_operation_root,
                &batch.staging_fingerprint,
                error.into(),
            );
        }
        let mut installed = Vec::with_capacity(batch.items.len());
        for (index, item) in batch.items.iter().enumerate() {
            match self.filesystem.install_staged_skill(
                &item.staged_source.staged_content_root,
                &item.final_entity_path,
                &self.library_root,
                &batch.operation_id,
                &item.tree_snapshot,
            ) {
                Ok(fingerprint) => {
                    journal.items[index].installed_fingerprint = Some(fingerprint.clone());
                    journal.phase = FileImportJournalPhase::FileSystemApplied;
                    installed.push((item.final_entity_path.clone(), fingerprint));
                    if let Err(error) = self
                        .filesystem
                        .write_file_import_journal(&self.library_root, &journal)
                    {
                        let rollback = self.rollback_installed_paths(&installed);
                        let cleanup = self.filesystem.discard_staging(
                            &batch.staging_operation_root,
                            &self.library_root,
                            Some(&batch.staging_fingerprint),
                        );
                        if rollback.is_ok() && cleanup.is_ok() {
                            return self
                                .finish_aborted_file_journal(&batch.operation_id, error.into());
                        }
                        let retry = self
                            .filesystem
                            .write_file_import_journal(&self.library_root, &journal);
                        return Err(ImportError::RecoveryRequired(format!(
                            "file Import batch journal progress failed: {error}; rollback: {rollback:?}; staging cleanup: {cleanup:?}; journal retry: {retry:?}"
                        )));
                    }
                }
                Err(error @ FileSystemError::RecoveryRequired { .. }) => {
                    return Err(ImportError::RecoveryRequired(error.to_string()));
                }
                Err(FileSystemError::PlanStale { .. }) => {
                    return self.rollback_file_batch(&batch, &installed, ImportError::PlanStale);
                }
                Err(error) => {
                    return self.rollback_file_batch(
                        &batch,
                        &installed,
                        ImportError::FileSystem(error),
                    );
                }
            }
        }
        if let Err(error) = self.filesystem.discard_staging(
            &batch.staging_operation_root,
            &self.library_root,
            Some(&batch.staging_fingerprint),
        ) {
            return Err(ImportError::RecoveryRequired(format!(
                "file Import batch staging cleanup failed after filesystem apply: {error}"
            )));
        }
        let records = batch
            .items
            .iter()
            .map(file_record_from_plan)
            .collect::<Vec<_>>();
        let snapshot_version = match self.store.insert_files(records) {
            Ok(version) => version,
            Err(error) => {
                return match self.rollback_installed_paths(&installed) {
                    Ok(()) => self.finish_aborted_file_journal(&batch.operation_id, error.into()),
                    Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                        "catalog batch insert failed: {error}; filesystem rollback also failed: {compensation}"
                    ))),
                };
            }
        };
        journal.phase = FileImportJournalPhase::CatalogCommitted;
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return Err(ImportError::RecoveryRequired(format!(
                "file Import batch committed the catalog, but journal progress could not be persisted: {error}"
            )));
        }
        if let Err(error) = self
            .filesystem
            .finish_file_import_journal(&self.library_root, &batch.operation_id)
        {
            return Err(ImportError::RecoveryRequired(format!(
                "file Import batch committed, but its journal could not be archived: {error}"
            )));
        }
        let results = batch
            .items
            .into_iter()
            .map(|item| FileImportResult {
                operation_id: batch.operation_id.clone(),
                skill_id: item.skill_id,
                directory_name: item.candidate.directory_name,
                final_entity_path: item.final_entity_path,
                snapshot_version,
                reinstalled: false,
            })
            .collect();
        Ok(FileImportSelectionResult {
            operation_id: batch.operation_id,
            items: results,
            snapshot_version,
        })
    }

    fn rollback_file_batch<T>(
        &self,
        batch: &PlannedFileImportBatch,
        installed: &[(PathBuf, DirectoryFingerprint)],
        original: ImportError,
    ) -> Result<T, ImportError> {
        let rollback = self.rollback_installed_paths(installed);
        let cleanup = self.filesystem.discard_staging(
            &batch.staging_operation_root,
            &self.library_root,
            Some(&batch.staging_fingerprint),
        );
        match (rollback, cleanup) {
            (Ok(()), Ok(())) => self.finish_aborted_file_journal(&batch.operation_id, original),
            (rollback, cleanup) => Err(ImportError::RecoveryRequired(format!(
                "{original}; batch rollback: {rollback:?}; staging cleanup: {cleanup:?}"
            ))),
        }
    }

    fn rollback_installed_paths(
        &self,
        installed: &[(PathBuf, DirectoryFingerprint)],
    ) -> Result<(), FileSystemError> {
        for (path, fingerprint) in installed.iter().rev() {
            self.filesystem
                .discard_installed_skill(path, &self.library_root, fingerprint)?;
        }
        Ok(())
    }

    pub fn plan_file(&self, source_path: &Path) -> Result<FileImportPreview, ImportError> {
        self.ensure_writes_ready()?;
        let operation_id = self.next_file_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let (staged_candidates, staging_fingerprint, _) =
            self.stage_and_validate_file(source_path, &staging_operation_root)?;
        if staged_candidates.len() != 1 {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::Validation(format!(
                    "file Import discovered {} Skills; use candidate selection",
                    staged_candidates.len()
                )),
            );
        }
        let (candidate, staged_source, tree_snapshot) = staged_candidates
            .into_iter()
            .next()
            .expect("a single staged candidate was checked");
        let required_space = tree_snapshot
            .total_file_bytes
            .saturating_mul(2)
            .saturating_add(DISK_SPACE_RESERVE_BYTES);
        let available_space = match self.filesystem.available_space(&self.library_root) {
            Ok(space) => space,
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        if available_space < required_space {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::DiskFull {
                    required_bytes: required_space,
                    available_bytes: available_space,
                },
            );
        }
        let conflict = match self.store.find_library_conflict(&candidate.identity_key) {
            Ok(conflict) => conflict.map(map_library_conflict),
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        let final_entity_path = self
            .library_root
            .join("skills")
            .join(&candidate.directory_name);
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("file-import-plan-{plan_number}");
        let skill_id = SkillId(format!(
            "file-{}-{}",
            self.clock.unix_epoch_nanos(),
            self.next_skill_id.fetch_add(1, Ordering::Relaxed)
        ));
        let plan = PlannedFileImport {
            candidate: candidate.clone(),
            staged_source,
            staging_operation_root,
            staging_fingerprint,
            final_entity_path: final_entity_path.clone(),
            skill_id,
            tree_snapshot,
            conflict: conflict.clone(),
            operation_id,
            created_at_millis: self.clock.monotonic_millis(),
            reinstall: None,
        };
        let journal = file_import_journal(
            &plan.operation_id,
            &plan.staging_operation_root,
            &plan.staging_fingerprint,
            std::slice::from_ref(&plan),
        );
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                error.into(),
            );
        }
        let mut file_plans = match self.file_plans.lock() {
            Ok(plans) => plans,
            Err(_) => {
                return self.abort_unapplied_file_journal(
                    &plan.operation_id,
                    &plan.staging_operation_root,
                    &plan.staging_fingerprint,
                    ImportError::Internal("file Import plan lock poisoned".into()),
                );
            }
        };
        file_plans.insert(plan_token.clone(), plan);
        Ok(FileImportPreview {
            plan_token,
            directory_name: candidate.directory_name,
            display_name: candidate.display_name,
            description: candidate.description,
            original_path: candidate.original_path,
            original_filename: candidate.original_filename,
            final_entity_path,
            can_apply: conflict.is_none(),
            conflict,
        })
    }

    pub fn plan_file_reinstall(
        &self,
        source_path: &Path,
    ) -> Result<FileImportPreview, ImportError> {
        self.ensure_writes_ready()?;
        let operation_id = self.next_file_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let (staged_candidates, staging_fingerprint, _) =
            self.stage_and_validate_file(source_path, &staging_operation_root)?;
        if staged_candidates.len() != 1 {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::Validation(
                    "file reinstall requires exactly one selected Skill".into(),
                ),
            );
        }
        let (candidate, staged_source, tree_snapshot) = staged_candidates
            .into_iter()
            .next()
            .expect("a single staged reinstall candidate was checked");
        let required_space = tree_snapshot
            .total_file_bytes
            .saturating_mul(2)
            .saturating_add(DISK_SPACE_RESERVE_BYTES);
        let available_space = match self.filesystem.available_space(&self.library_root) {
            Ok(space) => space,
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        if available_space < required_space {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::DiskFull {
                    required_bytes: required_space,
                    available_bytes: available_space,
                },
            );
        }
        let existing_record = match self.store.load_file_install(&candidate.identity_key) {
            Ok(Some(record)) => record,
            Ok(None) => {
                let error = match self.store.find_library_conflict(&candidate.identity_key) {
                    Ok(Some(conflict)) => ImportError::Conflict(conflict.directory_name),
                    Ok(None) => ImportError::Validation(format!(
                        "'{}' is not an existing file Install",
                        candidate.directory_name
                    )),
                    Err(error) => ImportError::Store(error),
                };
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error,
                );
            }
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        let existing_tree_snapshot = match self
            .filesystem
            .staged_tree_snapshot(&existing_record.final_entity_path)
        {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    ImportError::PlanStale,
                );
            }
        };
        let final_entity_path = existing_record.final_entity_path.clone();
        let activations = match self
            .store
            .desired_activations_for_skill(&existing_record.skill_id)
        {
            Ok(activations) => activations,
            Err(error) => {
                return self.cleanup_staging_after_error(
                    &staging_operation_root,
                    Some(&staging_fingerprint),
                    error.into(),
                );
            }
        };
        if self
            .verify_file_reinstall_activations(
                &final_entity_path,
                &existing_tree_snapshot.root,
                &activations,
            )
            .is_err()
        {
            return self.cleanup_staging_after_error(
                &staging_operation_root,
                Some(&staging_fingerprint),
                ImportError::PlanStale,
            );
        }
        let plan_token = format!(
            "file-reinstall-plan-{}",
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        );
        let plan = PlannedFileImport {
            candidate: candidate.clone(),
            staged_source,
            staging_operation_root,
            staging_fingerprint,
            final_entity_path: final_entity_path.clone(),
            skill_id: existing_record.skill_id.clone(),
            tree_snapshot,
            conflict: None,
            operation_id,
            created_at_millis: self.clock.monotonic_millis(),
            reinstall: Some(PlannedFileReinstall {
                existing_record,
                existing_tree_snapshot,
                activations,
            }),
        };
        let journal = file_import_journal(
            &plan.operation_id,
            &plan.staging_operation_root,
            &plan.staging_fingerprint,
            std::slice::from_ref(&plan),
        );
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                error.into(),
            );
        }
        let mut file_plans = match self.file_plans.lock() {
            Ok(plans) => plans,
            Err(_) => {
                return self.abort_unapplied_file_journal(
                    &plan.operation_id,
                    &plan.staging_operation_root,
                    &plan.staging_fingerprint,
                    ImportError::Internal("file reinstall plan lock poisoned".into()),
                );
            }
        };
        file_plans.insert(plan_token.clone(), plan);
        Ok(FileImportPreview {
            plan_token,
            directory_name: candidate.directory_name,
            display_name: candidate.display_name,
            description: candidate.description,
            original_path: candidate.original_path,
            original_filename: candidate.original_filename,
            final_entity_path,
            conflict: None,
            can_apply: true,
        })
    }

    pub fn apply_file(&self, plan_token: &str) -> Result<FileImportResult, ImportError> {
        self.ensure_writes_ready()?;
        let plan = self
            .file_plans
            .lock()
            .map_err(|_| ImportError::Internal("file Import plan lock poisoned".into()))?
            .remove(plan_token)
            .ok_or(ImportError::PlanNotFound)?;
        if self
            .clock
            .monotonic_millis()
            .saturating_sub(plan.created_at_millis)
            >= self.plan_ttl.as_millis()
        {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                ImportError::PlanNotFound,
            );
        }
        if let Some(conflict) = plan.conflict {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                ImportError::Conflict(conflict.directory_name),
            );
        }
        if let Some(reinstall) = &plan.reinstall {
            let current = match self.store.load_file_install(&plan.candidate.identity_key) {
                Ok(Some(current)) => current,
                Ok(None) => {
                    return self.abort_unapplied_file_journal(
                        &plan.operation_id,
                        &plan.staging_operation_root,
                        &plan.staging_fingerprint,
                        ImportError::PlanStale,
                    );
                }
                Err(error) => {
                    return self.abort_unapplied_file_journal(
                        &plan.operation_id,
                        &plan.staging_operation_root,
                        &plan.staging_fingerprint,
                        error.into(),
                    );
                }
            };
            let current_tree_snapshot = match self
                .filesystem
                .staged_tree_snapshot(&plan.final_entity_path)
            {
                Ok(snapshot) => snapshot,
                Err(_) => {
                    return self.abort_unapplied_file_journal(
                        &plan.operation_id,
                        &plan.staging_operation_root,
                        &plan.staging_fingerprint,
                        ImportError::PlanStale,
                    );
                }
            };
            let current_activations = match self.store.desired_activations_for_skill(&plan.skill_id)
            {
                Ok(activations) => activations,
                Err(error) => {
                    return self.abort_unapplied_file_journal(
                        &plan.operation_id,
                        &plan.staging_operation_root,
                        &plan.staging_fingerprint,
                        error.into(),
                    );
                }
            };
            if current != reinstall.existing_record
                || current_tree_snapshot != reinstall.existing_tree_snapshot
                || current_activations != reinstall.activations
                || self
                    .verify_file_reinstall_activations(
                        &plan.final_entity_path,
                        &current_tree_snapshot.root,
                        &current_activations,
                    )
                    .is_err()
            {
                return self.abort_unapplied_file_journal(
                    &plan.operation_id,
                    &plan.staging_operation_root,
                    &plan.staging_fingerprint,
                    ImportError::PlanStale,
                );
            }
        } else if let Some(conflict) = match self
            .store
            .find_library_conflict(&plan.candidate.identity_key)
        {
            Ok(conflict) => conflict,
            Err(error) => {
                return self.abort_unapplied_file_journal(
                    &plan.operation_id,
                    &plan.staging_operation_root,
                    &plan.staging_fingerprint,
                    error.into(),
                );
            }
        } {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                ImportError::Conflict(conflict.directory_name),
            );
        }
        let current_snapshot = match self
            .filesystem
            .staged_tree_snapshot(&plan.staged_source.staged_content_root)
        {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return self.abort_unapplied_file_journal(
                    &plan.operation_id,
                    &plan.staging_operation_root,
                    &plan.staging_fingerprint,
                    ImportError::PlanStale,
                );
            }
        };
        if current_snapshot != plan.tree_snapshot {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                ImportError::PlanStale,
            );
        }
        let mut journal = file_import_journal(
            &plan.operation_id,
            &plan.staging_operation_root,
            &plan.staging_fingerprint,
            std::slice::from_ref(&plan),
        );
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return self.abort_unapplied_file_journal(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
                error.into(),
            );
        }
        let applied = match &plan.reinstall {
            Some(reinstall) => self
                .filesystem
                .replace_staged_skill(
                    &plan.staged_source.staged_content_root,
                    &plan.final_entity_path,
                    &self.library_root,
                    &plan.operation_id,
                    &plan.tree_snapshot,
                    &reinstall.existing_tree_snapshot,
                )
                .map(AppliedFileChange::Replacement),
            None => self
                .filesystem
                .install_staged_skill(
                    &plan.staged_source.staged_content_root,
                    &plan.final_entity_path,
                    &self.library_root,
                    &plan.operation_id,
                    &plan.tree_snapshot,
                )
                .map(AppliedFileChange::New),
        };
        let applied = match applied {
            Ok(applied) => applied,
            Err(error @ FileSystemError::RecoveryRequired { .. }) => {
                return Err(ImportError::RecoveryRequired(error.to_string()));
            }
            Err(error) => {
                self.filesystem.discard_staging(
                    &plan.staging_operation_root,
                    &self.library_root,
                    Some(&plan.staging_fingerprint),
                )?;
                let original = match error {
                    FileSystemError::PlanStale { .. } => ImportError::PlanStale,
                    FileSystemError::Io { ref source, .. }
                        if source.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        ImportError::PlanStale
                    }
                    error => error.into(),
                };
                return self.finish_aborted_file_journal(&plan.operation_id, original);
            }
        };
        record_applied_change(&mut journal.items[0], &applied);
        journal.phase = FileImportJournalPhase::FileSystemApplied;
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            let rollback = self.rollback_applied_file(&plan, &applied);
            let cleanup = self.filesystem.discard_staging(
                &plan.staging_operation_root,
                &self.library_root,
                Some(&plan.staging_fingerprint),
            );
            if rollback.is_ok() && cleanup.is_ok() {
                return self.finish_aborted_file_journal(&plan.operation_id, error.into());
            }
            let retry = self
                .filesystem
                .write_file_import_journal(&self.library_root, &journal);
            return Err(ImportError::RecoveryRequired(format!(
                "file Import journal progress failed: {error}; rollback: {rollback:?}; staging cleanup: {cleanup:?}; journal retry: {retry:?}"
            )));
        }
        if let Err(error) = self.filesystem.discard_staging(
            &plan.staging_operation_root,
            &self.library_root,
            Some(&plan.staging_fingerprint),
        ) {
            return match self.rollback_applied_file(&plan, &applied) {
                Ok(()) => {
                    journal.phase = FileImportJournalPhase::Planned;
                    journal.items[0].installed_fingerprint = None;
                    journal.items[0].replacement = None;
                    let persisted = self
                        .filesystem
                        .write_file_import_journal(&self.library_root, &journal);
                    Err(ImportError::RecoveryRequired(format!(
                        "staging cleanup failed after filesystem apply: {error}; rollback succeeded; recovery journal reset: {persisted:?}"
                    )))
                }
                Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                    "staging cleanup failed: {error}; installed Skill rollback also failed: {compensation}"
                ))),
            };
        }
        if let (Some(reinstall), AppliedFileChange::Replacement(replacement)) =
            (&plan.reinstall, &applied)
        {
            if let Err(error) = self.verify_file_reinstall_activations(
                &plan.final_entity_path,
                &replacement.installed_fingerprint,
                &reinstall.activations,
            ) {
                return match self.rollback_applied_file(&plan, &applied) {
                    Ok(()) => self.finish_aborted_file_journal(&plan.operation_id, error),
                    Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                        "file reinstall Activation verification failed; stable entity rollback also failed: {compensation}"
                    ))),
                };
            }
        }
        let record = file_record_from_plan(&plan);
        let catalog_result = if plan.reinstall.is_some() {
            self.store.replace_file(record)
        } else {
            self.store.insert_file(record)
        };
        let snapshot_version = match catalog_result {
            Ok(version) => version,
            Err(error) => {
                return match self.rollback_applied_file(&plan, &applied) {
                    Ok(()) => self.finish_aborted_file_journal(&plan.operation_id, error.into()),
                    Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                        "catalog insert failed: {error}; installed Skill rollback also failed: {compensation}"
                    ))),
                };
            }
        };
        journal.phase = FileImportJournalPhase::CatalogCommitted;
        if let Err(error) = self
            .filesystem
            .write_file_import_journal(&self.library_root, &journal)
        {
            return Err(ImportError::RecoveryRequired(format!(
                "file Import committed the catalog, but journal progress could not be persisted: {error}"
            )));
        }
        if let AppliedFileChange::Replacement(replacement) = &applied {
            if let Err(error) = self
                .filesystem
                .commit_replaced_skill(replacement, &self.library_root)
            {
                return Err(ImportError::RecoveryRequired(format!(
                    "file reinstall committed, but backup cleanup failed: {error}"
                )));
            }
        }
        if let Err(error) = self
            .filesystem
            .finish_file_import_journal(&self.library_root, &plan.operation_id)
        {
            return Err(ImportError::RecoveryRequired(format!(
                "file Import committed, but its journal could not be archived: {error}"
            )));
        }
        let reinstalled = plan.reinstall.is_some();
        Ok(FileImportResult {
            operation_id: plan.operation_id,
            skill_id: plan.skill_id,
            directory_name: plan.candidate.directory_name,
            final_entity_path: plan.final_entity_path,
            snapshot_version,
            reinstalled,
        })
    }

    fn rollback_applied_file(
        &self,
        plan: &PlannedFileImport,
        applied: &AppliedFileChange,
    ) -> Result<(), FileSystemError> {
        match applied {
            AppliedFileChange::New(fingerprint) => self.filesystem.discard_installed_skill(
                &plan.final_entity_path,
                &self.library_root,
                fingerprint,
            ),
            AppliedFileChange::Replacement(replacement) => self
                .filesystem
                .rollback_replaced_skill(replacement, &self.library_root),
        }
    }

    fn verify_file_reinstall_activations(
        &self,
        final_entity_path: &Path,
        expected_entity: &DirectoryFingerprint,
        activations: &[DesiredActivation],
    ) -> Result<(), ImportError> {
        let current_entity = self
            .filesystem
            .directory_fingerprint(final_entity_path)
            .map_err(|_| ImportError::PlanStale)?;
        if current_entity != *expected_entity {
            return Err(ImportError::PlanStale);
        }
        for activation in activations {
            if !matches!(
                self.filesystem
                    .activation_snapshot(&activation.expected_entry_path),
                Ok(crate::seams::filesystem::ActivationEntrySnapshot::Symlink { target })
                    if target == activation.expected_target_path
            ) {
                return Err(ImportError::PlanStale);
            }
            let resolved_target = self
                .filesystem
                .directory_fingerprint(&activation.expected_target_path)
                .map_err(|_| ImportError::PlanStale)?;
            if resolved_target != *expected_entity {
                return Err(ImportError::PlanStale);
            }
        }
        Ok(())
    }

    fn finish_aborted_file_journal<T>(
        &self,
        operation_id: &str,
        original: ImportError,
    ) -> Result<T, ImportError> {
        match self
            .filesystem
            .finish_file_import_journal(&self.library_root, operation_id)
        {
            Ok(()) => Err(original),
            Err(error) => Err(ImportError::RecoveryRequired(format!(
                "{original}; aborted operation journal cleanup also failed: {error}"
            ))),
        }
    }

    fn abort_unapplied_file_journal<T>(
        &self,
        operation_id: &str,
        staging_operation_root: &Path,
        staging_fingerprint: &DirectoryFingerprint,
        original: ImportError,
    ) -> Result<T, ImportError> {
        let cleanup = self.filesystem.discard_staging(
            staging_operation_root,
            &self.library_root,
            Some(staging_fingerprint),
        );
        let finish = self
            .filesystem
            .finish_file_import_journal(&self.library_root, operation_id);
        match (cleanup, finish) {
            (Ok(()), Ok(())) => Err(original),
            (cleanup, finish) => Err(ImportError::RecoveryRequired(format!(
                "{original}; staging cleanup: {cleanup:?}; journal cleanup: {finish:?}"
            ))),
        }
    }

    pub fn cancel_file(&self, plan_token: &str) -> Result<bool, ImportError> {
        self.ensure_writes_ready()?;
        let plan = self
            .file_plans
            .lock()
            .map_err(|_| ImportError::Internal("file Import plan lock poisoned".into()))?
            .remove(plan_token);
        if let Some(plan) = plan {
            self.finish_cancelled_file_plan(
                &plan.operation_id,
                &plan.staging_operation_root,
                &plan.staging_fingerprint,
            )?;
            return Ok(true);
        }
        let batch = self
            .file_batch_plans
            .lock()
            .map_err(|_| ImportError::Internal("file Import batch plan lock poisoned".into()))?
            .remove(plan_token);
        if let Some(batch) = batch {
            self.finish_cancelled_file_plan(
                &batch.operation_id,
                &batch.staging_operation_root,
                &batch.staging_fingerprint,
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    fn finish_cancelled_file_plan(
        &self,
        operation_id: &str,
        staging_operation_root: &Path,
        staging_fingerprint: &DirectoryFingerprint,
    ) -> Result<(), ImportError> {
        let cleanup = self.filesystem.discard_staging(
            staging_operation_root,
            &self.library_root,
            Some(staging_fingerprint),
        );
        let finish = self
            .filesystem
            .finish_file_import_journal(&self.library_root, operation_id);
        match (cleanup, finish) {
            (Ok(()), Ok(())) => Ok(()),
            (cleanup, finish) => Err(ImportError::RecoveryRequired(format!(
                "cancelled file Import cleanup failed: staging={cleanup:?}; journal={finish:?}"
            ))),
        }
    }

    fn stage_and_validate_file(
        &self,
        source_path: &Path,
        staging_operation_root: &Path,
    ) -> Result<ValidatedFileSource, ImportError> {
        self.stage_and_validate_file_selection(source_path, staging_operation_root, None)
    }

    fn stage_and_validate_file_selection(
        &self,
        source_path: &Path,
        staging_operation_root: &Path,
        selected_directory_names: Option<&HashSet<String>>,
    ) -> Result<ValidatedFileSource, ImportError> {
        let estimated_bytes = self.file_source.estimated_size(source_path)?;
        let required_space = estimated_bytes
            .saturating_mul(2)
            .saturating_add(DISK_SPACE_RESERVE_BYTES);
        let available_space = self.filesystem.available_space(&self.library_root)?;
        if available_space < required_space {
            return Err(ImportError::DiskFull {
                required_bytes: required_space,
                available_bytes: available_space,
            });
        }
        let staged_source = self
            .file_source
            .stage(source_path, staging_operation_root)?;
        let staging_fingerprint = self
            .filesystem
            .directory_fingerprint(staging_operation_root)?;
        let validation = (|| {
            let (discovered, truncated) =
                discover_staged_skills(self.filesystem.as_ref(), &staged_source)?;
            if discovered.is_empty() {
                return Err(ImportError::Validation(
                    "file Import source must contain a readable SKILL.md".into(),
                ));
            }
            let mut candidates = Vec::with_capacity(discovered.len());
            for (directory_name, staged_skill_path) in discovered {
                if selected_directory_names
                    .is_some_and(|selected| !selected.contains(&directory_name))
                {
                    continue;
                }
                let identity_key = normalize_identity(&directory_name)?;
                let tree_snapshot = self.filesystem.staged_tree_snapshot(&staged_skill_path)?;
                validate_staged_tree(self.filesystem.as_ref(), &tree_snapshot)?;
                let skill_markdown = self
                    .filesystem
                    .read_skill_document(&staged_skill_path)
                    .map_err(|error| {
                        ImportError::Validation(format!(
                            "file Import SKILL.md is not readable: {error}"
                        ))
                    })?;
                let metadata = parse_skill_metadata(&skill_markdown);
                let candidate = FileImportCandidate {
                    directory_name: directory_name.clone(),
                    identity_key,
                    display_name: metadata.name.clone().unwrap_or(directory_name),
                    description: metadata.description.unwrap_or_default(),
                    frontmatter_name: metadata.name,
                    original_path: staged_source.original_path.clone(),
                    original_filename: staged_source.original_filename.clone(),
                };
                candidates.push((
                    candidate,
                    StagedFileSource {
                        staged_content_root: staged_skill_path,
                        ..staged_source.clone()
                    },
                    tree_snapshot,
                ));
            }
            Ok((candidates, truncated))
        })();
        match validation {
            Ok((candidates, truncated)) => Ok((candidates, staging_fingerprint, truncated)),
            Err(error) => match self.filesystem.discard_staging(
                staging_operation_root,
                &self.library_root,
                Some(&staging_fingerprint),
            ) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(ImportError::RecoveryRequired(format!(
                    "{error}; staging cleanup also failed: {cleanup}"
                ))),
            },
        }
    }

    fn cleanup_staging_after_error<T>(
        &self,
        staging_operation_root: &Path,
        expected: Option<&DirectoryFingerprint>,
        original: ImportError,
    ) -> Result<T, ImportError> {
        match self
            .filesystem
            .discard_staging(staging_operation_root, &self.library_root, expected)
        {
            Ok(()) => Err(original),
            Err(cleanup) => Err(ImportError::RecoveryRequired(format!(
                "{original}; staging cleanup also failed: {cleanup}"
            ))),
        }
    }

    fn next_file_operation_id(&self) -> String {
        format!(
            "file-import-{}-{}",
            self.clock.unix_epoch_nanos(),
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn ensure_writes_ready(&self) -> Result<(), ImportError> {
        if self.recovery_gate.writes_are_ready() {
            Ok(())
        } else {
            Err(ImportError::RecoveryRequired(
                "startup recovery is still in progress".into(),
            ))
        }
    }
}

fn file_record_from_plan(plan: &PlannedFileImport) -> FileImportRecord {
    FileImportRecord {
        skill_id: plan.skill_id.clone(),
        directory_name: plan.candidate.directory_name.clone(),
        identity_key: plan.candidate.identity_key.clone(),
        display_name: plan.candidate.display_name.clone(),
        description: plan.candidate.description.clone(),
        library_entry_path: plan.final_entity_path.clone(),
        final_entity_path: plan.final_entity_path.clone(),
        recorded_content_hash: plan.tree_snapshot.content_hash.clone(),
        original_path: plan.candidate.original_path.clone(),
        original_filename: plan.candidate.original_filename.clone(),
    }
}

fn file_import_journal(
    operation_id: &str,
    staging_operation_root: &Path,
    staging_fingerprint: &DirectoryFingerprint,
    plans: &[PlannedFileImport],
) -> FileImportJournal {
    FileImportJournal {
        version: 1,
        operation_id: operation_id.to_owned(),
        phase: FileImportJournalPhase::Planned,
        staging_operation_root: staging_operation_root.to_path_buf(),
        staging_fingerprint: staging_fingerprint.clone(),
        items: plans
            .iter()
            .map(|plan| FileImportJournalItem {
                skill_id: plan.skill_id.0.clone(),
                final_entity_path: plan.final_entity_path.clone(),
                expected_content_hash: plan.tree_snapshot.content_hash.clone(),
                staged_root_fingerprint: plan.tree_snapshot.root.clone(),
                replacement_planned: plan.reinstall.is_some(),
                replacement_original_tree: plan
                    .reinstall
                    .as_ref()
                    .map(|reinstall| reinstall.existing_tree_snapshot.clone()),
                installed_fingerprint: None,
                replacement: None,
            })
            .collect(),
    }
}

fn record_applied_change(item: &mut FileImportJournalItem, applied: &AppliedFileChange) {
    match applied {
        AppliedFileChange::New(fingerprint) => {
            item.installed_fingerprint = Some(fingerprint.clone());
        }
        AppliedFileChange::Replacement(replacement) => {
            item.installed_fingerprint = Some(replacement.installed_fingerprint.clone());
            item.replacement = Some(replacement.clone());
        }
    }
}

fn discover_staged_skills(
    filesystem: &dyn FileSystem,
    staged: &StagedFileSource,
) -> Result<(Vec<(String, PathBuf)>, bool), ImportError> {
    let root = &staged.staged_content_root;
    let mut standard = Vec::new();
    if filesystem.staged_has_skill_document(root, "SKILL.md")? {
        standard.push((staged.suggested_root_name.clone(), root.clone()));
    } else {
        collect_immediate_skill_directories(filesystem, root, &mut standard)?;
        let skills_directory = root.join("skills");
        let root_directories = filesystem.staged_child_directories(root)?;
        if root_directories.contains(&skills_directory) {
            collect_immediate_skill_directories(filesystem, &skills_directory, &mut standard)?;
        }
        for possible_plugin in root_directories {
            if possible_plugin == skills_directory {
                continue;
            }
            let plugin_skills = possible_plugin.join("skills");
            if filesystem
                .staged_child_directories(&possible_plugin)?
                .contains(&plugin_skills)
            {
                collect_immediate_skill_directories(filesystem, &plugin_skills, &mut standard)?;
            }
        }
    }
    let mut discovered = if standard.is_empty() {
        let mut recursive = Vec::new();
        collect_recursive_skill_directories(filesystem, root, &mut recursive)?;
        recursive
    } else {
        standard
    };
    discovered.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    discovered.dedup_by(|left, right| left.1 == right.1);
    let truncated = discovered.len() > MAX_SOURCE_SKILLS;
    discovered.truncate(MAX_SOURCE_SKILLS);
    Ok((discovered, truncated))
}

fn collect_immediate_skill_directories(
    filesystem: &dyn FileSystem,
    directory: &Path,
    candidates: &mut Vec<(String, PathBuf)>,
) -> Result<(), ImportError> {
    for path in filesystem.staged_child_directories(directory)? {
        if filesystem.staged_has_skill_document(&path, "SKILL.md")? {
            let directory_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    ImportError::Validation("Skill directory names must be valid UTF-8".into())
                })?;
            candidates.push((directory_name, path));
        }
    }
    Ok(())
}

fn collect_recursive_skill_directories(
    filesystem: &dyn FileSystem,
    directory: &Path,
    candidates: &mut Vec<(String, PathBuf)>,
) -> Result<(), ImportError> {
    for path in filesystem.staged_child_directories(directory)? {
        if filesystem.staged_has_skill_document(&path, "SKILL.md")? {
            let directory_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .ok_or_else(|| {
                    ImportError::Validation("Skill directory names must be valid UTF-8".into())
                })?;
            candidates.push((directory_name, path));
            if candidates.len() > MAX_SOURCE_SKILLS {
                return Ok(());
            }
        } else {
            collect_recursive_skill_directories(filesystem, &path, candidates)?;
            if candidates.len() > MAX_SOURCE_SKILLS {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn validate_staged_tree(
    filesystem: &dyn FileSystem,
    snapshot: &StagedTreeSnapshot,
) -> Result<(), ImportError> {
    let mut paths = HashMap::new();
    let mut total_bytes = 0_u64;
    for entry in &snapshot.entries {
        if entry.relative_path.to_str().is_none() {
            return Err(ImportError::Validation(
                "file Import paths must be valid UTF-8".into(),
            ));
        }
        match &entry.kind {
            StagedEntryKind::Directory => {}
            StagedEntryKind::File { length } => {
                let limit = if entry.relative_path == Path::new("SKILL.md") {
                    MAX_SKILL_DOCUMENT_BYTES
                } else {
                    MAX_FILE_BYTES
                };
                if *length > limit {
                    return Err(ImportError::Validation(format!(
                        "file Import file exceeds its size limit: {}",
                        entry.relative_path.display()
                    )));
                }
                total_bytes = total_bytes.saturating_add(*length);
            }
            StagedEntryKind::Symlink { target } => {
                if target.to_str().is_none() || target.is_absolute() {
                    return Err(ImportError::Validation(format!(
                        "file Import symlink target must be a valid UTF-8 relative path: {}",
                        entry.relative_path.display()
                    )));
                }
            }
        }
        paths.insert(entry.relative_path.clone(), &entry.kind);
    }
    if !matches!(
        paths.get(Path::new("SKILL.md")),
        Some(StagedEntryKind::File { .. } | StagedEntryKind::Symlink { .. })
    ) {
        return Err(ImportError::Validation(
            "file Import source must contain a readable SKILL.md".into(),
        ));
    }
    if total_bytes > MAX_SKILL_BYTES || snapshot.total_file_bytes > MAX_SKILL_BYTES {
        return Err(ImportError::Validation(
            "file Import Skill exceeds the 256 MB size limit".into(),
        ));
    }
    for entry in &snapshot.entries {
        if matches!(entry.kind, StagedEntryKind::Symlink { .. }) {
            validate_staged_symlink(
                filesystem,
                &snapshot.root.canonical_path,
                entry.relative_path.as_path(),
                &paths,
            )?;
        }
    }
    Ok(())
}

fn validate_staged_symlink(
    filesystem: &dyn FileSystem,
    skill_root: &Path,
    link: &Path,
    entries: &HashMap<PathBuf, &StagedEntryKind>,
) -> Result<(), ImportError> {
    let canonical_target = filesystem
        .canonicalize_staged_path(&skill_root.join(link))
        .map_err(|_| unsafe_staged_symlink(link))?;
    if !canonical_target.starts_with(skill_root) {
        return Err(unsafe_staged_symlink(link));
    }
    let mut current = link.to_path_buf();
    let mut seen = HashSet::new();
    for _ in 0..16 {
        if !seen.insert(current.clone()) {
            return Err(unsafe_staged_symlink(link));
        }
        let Some(StagedEntryKind::Symlink { target }) = entries.get(&current) else {
            return if entries.contains_key(&current) {
                Ok(())
            } else {
                Err(unsafe_staged_symlink(link))
            };
        };
        current = resolve_staged_relative_target(
            current.parent().unwrap_or_else(|| Path::new("")),
            target,
        )?;
    }
    Err(unsafe_staged_symlink(link))
}

fn resolve_staged_relative_target(parent: &Path, target: &Path) -> Result<PathBuf, ImportError> {
    let mut segments = parent
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in target.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => segments.push(value.to_os_string()),
            std::path::Component::ParentDir => {
                if segments.pop().is_none() {
                    return Err(ImportError::Validation(
                        "file Import symlink escapes the Skill".into(),
                    ));
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(ImportError::Validation(
                    "file Import symlink target must be relative".into(),
                ));
            }
        }
    }
    Ok(segments.into_iter().collect())
}

fn unsafe_staged_symlink(link: &Path) -> ImportError {
    ImportError::Validation(format!(
        "file Import symlink escapes the Skill, is dangling, or is cyclic: {}",
        link.display()
    ))
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
    #[error(
        "file Import requires {required_bytes} bytes of free space, but only {available_bytes} bytes are available"
    )]
    DiskFull {
        required_bytes: u64,
        available_bytes: u64,
    },
    #[error("file Import requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Store(#[from] ImportStoreError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Source(#[from] SourceError),
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
