use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::{SkillId, parse_skill_metadata, skill_identity_key};
use crate::core::git_source::{
    DEFAULT_BRANCH_REF, DiscoveryMode, GitResolveError, GitSourceParseError, GitSourceSpec,
    ResolvedGitRef, discover_skills_from_paths, git_mirror_path, parse_git_source_input,
    repo_name_from_url, resolve_git_ref, skill_document_path, validate_skill_path,
};
use crate::core::write_gate::{PlanCheck, PlanTicket, WriteGate};
use crate::seams::activation_store::DesiredActivation;
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    DirectoryFingerprint, FileImportJournal, FileImportJournalItem, FileImportJournalPhase,
    FileReplacement, FileSystem, FileSystemError, LinkSourceEntryKind, LinkSourceSnapshot,
    SkillFingerprint, StagedEntryKind, StagedTreeSnapshot,
};
use crate::seams::import_store::{
    FileImportRecord, ImportStore, ImportStoreError, LibraryConflict as StoreLibraryConflict,
    LinkImportRecord, RemoteImportRecord, RemoteInstallRecord,
};
use crate::seams::source::{
    FileSource, GitSource, GitTreeEntryKind, SourceError, StagedFileSource,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportCandidate {
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    /// Repo-relative Skill directory; empty means the repo root.
    pub skill_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportDiscovery {
    pub repo_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub candidates: Vec<GitImportCandidate>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportPreview {
    pub plan_token: String,
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub final_entity_path: PathBuf,
    pub conflict: Option<LibraryConflict>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportSelectionPreview {
    pub plan_token: String,
    pub repo_url: String,
    pub requested_ref: String,
    pub resolved_commit: String,
    pub items: Vec<GitImportPreview>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportResult {
    pub operation_id: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitImportSelectionResult {
    pub operation_id: String,
    pub items: Vec<GitImportResult>,
    pub snapshot_version: u64,
}

#[derive(Clone)]
struct PlannedLinkImport {
    candidate: LinkImportCandidate,
    source_snapshot: LinkSourceSnapshot,
    skill_id: SkillId,
    fingerprint: SkillFingerprint,
    conflict: Option<LibraryConflict>,
    gate_generation: u64,
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
    gate_generation: u64,
    created_at_millis: u128,
    reinstall: Option<PlannedFileReinstall>,
    /// Present for remote Installs (Import selection or Update reinstall).
    remote: Option<PlannedRemoteImport>,
}

#[derive(Clone)]
struct PlannedFileReinstall {
    existing_record: ReinstallBaseline,
    existing_tree_snapshot: StagedTreeSnapshot,
    activations: Vec<DesiredActivation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ReinstallBaseline {
    File(FileImportRecord),
    Remote(RemoteInstallRecord),
}

impl ReinstallBaseline {
    fn recorded_content_hash(&self) -> &str {
        match self {
            Self::File(record) => &record.recorded_content_hash,
            Self::Remote(record) => &record.recorded_content_hash,
        }
    }

    /// Compare only the stable identity fields; derived observations such as
    /// persisted health may legitimately change between plan and apply.
    fn stable_matches(&self, other: &ReinstallBaseline) -> bool {
        match (self, other) {
            (Self::File(left), Self::File(right)) => {
                left.skill_id == right.skill_id
                    && left.identity_key == right.identity_key
                    && left.final_entity_path == right.final_entity_path
                    && left.recorded_content_hash == right.recorded_content_hash
            }
            (Self::Remote(left), Self::Remote(right)) => {
                left.skill_id == right.skill_id
                    && left.identity_key == right.identity_key
                    && left.final_entity_path == right.final_entity_path
                    && left.recorded_content_hash == right.recorded_content_hash
                    && left.source_url == right.source_url
                    && left.requested_ref == right.requested_ref
                    && left.verification_anchor_commit == right.verification_anchor_commit
                    && left.skill_path == right.skill_path
            }
            _ => false,
        }
    }
}

#[derive(Clone)]
struct PlannedRemoteImport {
    repo_url: String,
    requested_ref: String,
    resolved_commit: String,
    skill_path: String,
    abandon_changes: bool,
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
    gate_generation: u64,
    created_at_millis: u128,
    commit_remotes: bool,
}

pub struct ImportService {
    store: Arc<dyn ImportStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    file_source: Arc<dyn FileSource>,
    git_source: Arc<dyn GitSource>,
    library_root: PathBuf,
    git_cache_root: Option<PathBuf>,
    plans: Mutex<HashMap<String, PlannedLinkImport>>,
    file_plans: Mutex<HashMap<String, PlannedFileImport>>,
    file_batch_plans: Mutex<HashMap<String, PlannedFileImportBatch>>,
    next_plan_id: AtomicU64,
    next_skill_id: AtomicU64,
    plan_ttl: Duration,
    write_gate: Arc<WriteGate>,
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
            git_source: Arc::new(crate::adapters::git_source::SystemGitSource::new()),
            library_root,
            git_cache_root: None,
            plans: Mutex::new(HashMap::new()),
            file_plans: Mutex::new(HashMap::new()),
            file_batch_plans: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            next_skill_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
            write_gate: Arc::new(WriteGate::open_for_tests()),
        }
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.write_gate = write_gate;
        self
    }

    pub fn with_git_source(mut self, git_source: Arc<dyn GitSource>) -> Self {
        self.git_source = git_source;
        self
    }

    pub fn with_git_cache_root(mut self, git_cache_root: PathBuf) -> Self {
        self.git_cache_root = Some(git_cache_root);
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
                gate_generation: self.write_gate.generation(),
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
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ImportError::PlanStale);
        }
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
                gate_generation: self.write_gate.generation(),
                created_at_millis,
                reinstall: None,
                remote: None,
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
            gate_generation: self.write_gate.generation(),
            created_at_millis,
            commit_remotes: false,
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
        if self.write_gate.check_plan(PlanTicket {
            generation: batch.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ImportError::PlanStale);
        }
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
        let snapshot_version = if batch.commit_remotes {
            let mut records = Vec::with_capacity(batch.items.len());
            for item in &batch.items {
                let remote = item
                    .remote
                    .as_ref()
                    .expect("git Import batch items carry a remote record");
                let canonical =
                    crate::adapters::remote_provider::normalize_catalog_url(&remote.repo_url)
                        .unwrap_or_else(|_| remote.repo_url.clone());
                let remote_id = self.remote_id_for(&canonical)?;
                records.push(remote_record_from_plan(item, remote, remote_id, true, None));
            }
            match self.store.insert_remotes(records) {
                Ok(version) => version,
                Err(error) => {
                    return match self.rollback_installed_paths(&installed) {
                        Ok(()) => {
                            self.finish_aborted_file_journal(&batch.operation_id, error.into())
                        }
                        Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                            "catalog batch insert failed: {error}; filesystem rollback also failed: {compensation}"
                        ))),
                    };
                }
            }
        } else {
            let records = batch
                .items
                .iter()
                .map(file_record_from_plan)
                .collect::<Vec<_>>();
            match self.store.insert_files(records) {
                Ok(version) => version,
                Err(error) => {
                    return match self.rollback_installed_paths(&installed) {
                        Ok(()) => {
                            self.finish_aborted_file_journal(&batch.operation_id, error.into())
                        }
                        Err(compensation) => Err(ImportError::RecoveryRequired(format!(
                            "catalog batch insert failed: {error}; filesystem rollback also failed: {compensation}"
                        ))),
                    };
                }
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
            gate_generation: self.write_gate.generation(),
            created_at_millis: self.clock.monotonic_millis(),
            reinstall: None,
            remote: None,
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
            gate_generation: self.write_gate.generation(),
            created_at_millis: self.clock.monotonic_millis(),
            reinstall: Some(PlannedFileReinstall {
                existing_record: ReinstallBaseline::File(existing_record),
                existing_tree_snapshot,
                activations,
            }),
            remote: None,
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
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ImportError::PlanStale);
        }
        self.apply_planned_file(plan)
    }

    /// Apply a planned single-Skill Import or reinstall. Remote Updates set
    /// `abandon_changes` on the plan so Modified entities are never silently
    /// replaced.
    fn apply_planned_file(&self, plan: PlannedFileImport) -> Result<FileImportResult, ImportError> {
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
            let current = match &reinstall.existing_record {
                ReinstallBaseline::File(_) => self
                    .store
                    .load_file_install(&plan.candidate.identity_key)
                    .map(|record| record.map(ReinstallBaseline::File)),
                ReinstallBaseline::Remote(_) => self
                    .store
                    .load_remote_install(&plan.candidate.identity_key)
                    .map(|record| record.map(ReinstallBaseline::Remote)),
            };
            let current = match current {
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
            if !reinstall.existing_record.stable_matches(&current)
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
            if let Some(remote) = &plan.remote {
                if !remote.abandon_changes
                    && current_tree_snapshot.content_hash
                        != reinstall.existing_record.recorded_content_hash()
                {
                    return self.abort_unapplied_file_journal(
                        &plan.operation_id,
                        &plan.staging_operation_root,
                        &plan.staging_fingerprint,
                        ImportError::Modified,
                    );
                }
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
        let catalog_result = match (&plan.reinstall, &plan.remote) {
            (Some(_), Some(remote)) => {
                let (remote_id, original_commit_known, provider_hash) = match &plan.reinstall {
                    Some(PlannedFileReinstall {
                        existing_record: ReinstallBaseline::Remote(existing),
                        ..
                    }) => (
                        existing.remote_id.clone(),
                        existing.original_commit_known,
                        existing.provider_hash.clone(),
                    ),
                    _ => (String::new(), false, None),
                };
                self.store.update_remote_install(remote_record_from_plan(
                    &plan,
                    remote,
                    remote_id,
                    original_commit_known,
                    provider_hash,
                ))
            }
            (Some(_), None) => self.store.replace_file(record),
            (None, Some(remote)) => {
                let canonical =
                    crate::adapters::remote_provider::normalize_catalog_url(&remote.repo_url)
                        .unwrap_or_else(|_| remote.repo_url.clone());
                let remote_id = self.remote_id_for(&canonical)?;
                self.store.insert_remotes(vec![remote_record_from_plan(
                    &plan, remote, remote_id, true, None,
                )])
            }
            (None, None) => self.store.insert_file(record),
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

    // -- Git remote Import (ADR-0004) --

    /// Discover Skill candidates in a remote repository, following the two-phase
    /// discovery semantics (standard positions, recursive fallback, optional
    /// forced full depth). Fetches into the persistent mirror cache.
    pub fn discover_git(
        &self,
        source: &str,
        force_full_depth: bool,
    ) -> Result<GitImportDiscovery, ImportError> {
        let (spec, mirror, resolved) = self.git_setup(source)?;
        let mut candidates =
            self.discover_git_candidates(&spec, &mirror, &resolved, force_full_depth)?;
        let truncated = candidates.len() > MAX_SOURCE_SKILLS;
        candidates.truncate(MAX_SOURCE_SKILLS);
        Ok(GitImportDiscovery {
            repo_url: spec.url,
            requested_ref: resolved.recorded_ref,
            resolved_commit: resolved.commit,
            candidates,
            truncated,
        })
    }

    /// Preview a multi-select of Git Skill candidates: re-fetch, stage each
    /// selected Skill from the resolved commit, validate, and persist a
    /// filesystem journal before the user confirms.
    pub fn plan_git_selection(
        &self,
        source: &str,
        force_full_depth: bool,
        selected_directory_names: &[String],
    ) -> Result<GitImportSelectionPreview, ImportError> {
        self.ensure_writes_ready()?;
        let (spec, mirror, resolved) = self.git_setup(source)?;
        let all = self.discover_git_candidates(&spec, &mirror, &resolved, force_full_depth)?;
        let selected = selected_directory_names
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let chosen = all
            .into_iter()
            .filter(|candidate| selected.contains(&candidate.directory_name))
            .collect::<Vec<_>>();
        if chosen.is_empty() || chosen.len() != selected.len() {
            return Err(ImportError::Validation(
                "git Import selection contains an unknown Skill name".into(),
            ));
        }
        let mut seen = HashSet::new();
        for candidate in &chosen {
            if !seen.insert(candidate.identity_key.clone()) {
                return Err(ImportError::Validation(format!(
                    "git Import selection contains duplicate Skill names: '{}'",
                    candidate.directory_name
                )));
            }
            validate_skill_path(&candidate.skill_path)?;
        }
        let operation_id = self.next_git_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let mut staged_sources = Vec::with_capacity(chosen.len());
        for candidate in &chosen {
            let destination = staging_operation_root.join(&candidate.directory_name);
            if let Err(error) = self.git_source.stage_skill(
                &mirror,
                &resolved.commit,
                &candidate.skill_path,
                &destination,
            ) {
                let cleanup = self.filesystem.discard_staging(
                    &staging_operation_root,
                    &self.library_root,
                    None,
                );
                return match cleanup {
                    Ok(()) => Err(error.into()),
                    Err(cleanup) => Err(ImportError::RecoveryRequired(format!(
                        "git Import staging failed: {error}; staging cleanup also failed: {cleanup}"
                    ))),
                };
            }
            staged_sources.push(StagedFileSource {
                original_path: mirror.clone(),
                original_filename: repo_name_from_url(&spec.url),
                suggested_root_name: candidate.directory_name.clone(),
                staged_content_root: destination,
            });
        }
        let (staged, staging_fingerprint) =
            self.validate_prestaged_sources(&staging_operation_root, staged_sources)?;
        let required_space = staged
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
        let plan_token = format!("git-import-selection-plan-{plan_number}");
        let created_at_millis = self.clock.monotonic_millis();
        let mut items = Vec::with_capacity(chosen.len());
        let mut previews = Vec::with_capacity(chosen.len());
        for ((candidate, staged_source, tree_snapshot), git_candidate) in
            staged.into_iter().zip(chosen.iter())
        {
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
                "git-{}-{}",
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
                gate_generation: self.write_gate.generation(),
                created_at_millis,
                reinstall: None,
                remote: Some(PlannedRemoteImport {
                    repo_url: spec.url.clone(),
                    requested_ref: resolved.recorded_ref.clone(),
                    resolved_commit: resolved.commit.clone(),
                    skill_path: git_candidate.skill_path.clone(),
                    abandon_changes: false,
                }),
            });
            previews.push(GitImportPreview {
                plan_token: plan_token.clone(),
                directory_name: candidate.directory_name,
                display_name: candidate.display_name,
                description: candidate.description,
                skill_path: git_candidate.skill_path.clone(),
                final_entity_path,
                conflict: conflict.clone(),
                can_apply: conflict.is_none(),
            });
        }
        let can_apply = previews.iter().all(|preview| preview.can_apply);
        let batch = PlannedFileImportBatch {
            items,
            staging_operation_root,
            staging_fingerprint,
            operation_id,
            gate_generation: self.write_gate.generation(),
            created_at_millis,
            commit_remotes: true,
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
                    ImportError::Internal("git Import batch plan lock poisoned".into()),
                );
            }
        };
        batch_plans.insert(plan_token.clone(), batch);
        Ok(GitImportSelectionPreview {
            plan_token,
            repo_url: spec.url,
            requested_ref: resolved.recorded_ref,
            resolved_commit: resolved.commit,
            items: previews,
            can_apply,
        })
    }

    pub fn apply_git_selection(
        &self,
        plan_token: &str,
    ) -> Result<GitImportSelectionResult, ImportError> {
        let result = self.apply_file_selection(plan_token)?;
        Ok(GitImportSelectionResult {
            operation_id: result.operation_id,
            snapshot_version: result.snapshot_version,
            items: result
                .items
                .into_iter()
                .map(|item| GitImportResult {
                    operation_id: item.operation_id,
                    skill_id: item.skill_id,
                    directory_name: item.directory_name,
                    final_entity_path: item.final_entity_path,
                    snapshot_version: item.snapshot_version,
                })
                .collect(),
        })
    }

    /// Preview replacing an existing remote Install with the content at
    /// `resolved_commit` (an Update, or a path reselection). Detects Modified
    /// entities so Apply can require abandoning local changes.
    pub fn plan_git_reinstall(
        &self,
        identity_key: &str,
        source_url: &str,
        resolved_commit: &str,
        new_skill_path: Option<&str>,
        abandon_changes: bool,
    ) -> Result<FileImportPreview, ImportError> {
        self.ensure_writes_ready()?;
        let record = self
            .store
            .load_remote_install(identity_key)?
            .ok_or_else(|| {
                ImportError::Validation(format!(
                    "'{identity_key}' is not an existing remote Install"
                ))
            })?;
        if record.source_url != source_url {
            return Err(ImportError::Validation(
                "the Update source does not match the installed remote source".into(),
            ));
        }
        let skill_path = new_skill_path
            .map(str::to_owned)
            .unwrap_or_else(|| record.skill_path.clone());
        validate_skill_path(&skill_path)?;
        let spec = GitSourceSpec {
            url: record.source_url.clone(),
            requested_ref: Some(record.requested_ref.clone()),
            path_prefix: None,
        };
        let mirror = self.git_mirror_for(&spec)?;
        let fetch = self.git_source.fetch_mirror(&spec.url, &mirror);
        let commit = match self.git_source.resolve_commit(&mirror, resolved_commit) {
            Ok(Some(commit)) => commit,
            Ok(None) => {
                return Err(ImportError::from(GitResolveError::RefNotFound(
                    resolved_commit.into(),
                )));
            }
            Err(error) => return Err(error.into()),
        };
        if let Err(error) = fetch {
            // A stale mirror is acceptable when the pinned commit is already
            // present; otherwise the fetch failure is the actionable error.
            if self.git_source.resolve_commit(&mirror, &commit)?.is_none() {
                return Err(error.into());
            }
        }
        let document_path = skill_document_path(&skill_path);
        let entries = self.git_source.list_tree(&mirror, &commit)?;
        if !entries
            .iter()
            .any(|entry| entry.path.to_string_lossy() == document_path)
        {
            return Err(ImportError::Validation(format!(
                "the upstream Skill path '{skill_path}' no longer exists at commit {commit}"
            )));
        }
        let operation_id = self.next_git_operation_id();
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let destination = staging_operation_root.join(&record.directory_name);
        if let Err(error) = self
            .git_source
            .stage_skill(&mirror, &commit, &skill_path, &destination)
        {
            let cleanup =
                self.filesystem
                    .discard_staging(&staging_operation_root, &self.library_root, None);
            return match cleanup {
                Ok(()) => Err(error.into()),
                Err(cleanup) => Err(ImportError::RecoveryRequired(format!(
                    "git reinstall staging failed: {error}; staging cleanup also failed: {cleanup}"
                ))),
            };
        }
        let staged_source = StagedFileSource {
            original_path: mirror.clone(),
            original_filename: repo_name_from_url(&spec.url),
            suggested_root_name: record.directory_name.clone(),
            staged_content_root: destination,
        };
        let (mut staged, staging_fingerprint) =
            self.validate_prestaged_sources(&staging_operation_root, vec![staged_source])?;
        let (candidate, staged_source, tree_snapshot) = staged.remove(0);
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
        let existing_tree_snapshot = match self
            .filesystem
            .staged_tree_snapshot(&record.final_entity_path)
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
        let activations = match self.store.desired_activations_for_skill(&record.skill_id) {
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
                &record.final_entity_path,
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
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("git-reinstall-plan-{plan_number}");
        let plan = PlannedFileImport {
            candidate,
            staged_source,
            staging_operation_root,
            staging_fingerprint,
            final_entity_path: record.final_entity_path.clone(),
            skill_id: record.skill_id.clone(),
            tree_snapshot,
            conflict: None,
            operation_id,
            gate_generation: self.write_gate.generation(),
            created_at_millis: self.clock.monotonic_millis(),
            reinstall: Some(PlannedFileReinstall {
                existing_record: ReinstallBaseline::Remote(record.clone()),
                existing_tree_snapshot,
                activations,
            }),
            remote: Some(PlannedRemoteImport {
                repo_url: spec.url.clone(),
                requested_ref: spec
                    .requested_ref
                    .clone()
                    .unwrap_or_else(|| DEFAULT_BRANCH_REF.into()),
                resolved_commit: commit,
                skill_path,
                abandon_changes,
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
                    ImportError::Internal("git reinstall plan lock poisoned".into()),
                );
            }
        };
        file_plans.insert(plan_token.clone(), plan);
        Ok(FileImportPreview {
            plan_token,
            directory_name: record.directory_name.clone(),
            display_name: record.display_name.clone(),
            description: record.description.clone(),
            original_path: record.final_entity_path.clone(),
            original_filename: repo_name_from_url(&spec.url),
            final_entity_path: record.final_entity_path.clone(),
            conflict: None,
            can_apply: true,
        })
    }

    /// Apply a planned remote reinstall. `abandon_changes` overrides the
    /// plan's flag, so the Modified confirmation happens at Apply time.
    pub fn apply_git_reinstall(
        &self,
        plan_token: &str,
        abandon_changes: bool,
    ) -> Result<FileImportResult, ImportError> {
        self.ensure_writes_ready()?;
        let mut plan = self
            .file_plans
            .lock()
            .map_err(|_| ImportError::Internal("git reinstall plan lock poisoned".into()))?
            .remove(plan_token)
            .ok_or(ImportError::PlanNotFound)?;
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ImportError::PlanStale);
        }
        if let Some(remote) = &mut plan.remote {
            remote.abandon_changes = abandon_changes;
        }
        self.apply_planned_file(plan)
    }

    fn git_setup(
        &self,
        source: &str,
    ) -> Result<(GitSourceSpec, PathBuf, ResolvedGitRef), ImportError> {
        self.ensure_writes_ready()?;
        let spec = parse_git_source_input(source)?;
        let mirror = self.git_mirror_for(&spec)?;
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)?;
        Ok((spec, mirror, resolved))
    }

    fn git_mirror_for(&self, spec: &GitSourceSpec) -> Result<PathBuf, ImportError> {
        let cache_root = self
            .git_cache_root
            .clone()
            .ok_or_else(|| ImportError::Internal("Git cache root is not configured".into()))?;
        Ok(git_mirror_path(&cache_root, &spec.url))
    }

    /// Resolve the parent id for a canonical URL: reuse the existing
    /// parent (ADR-0013 §4.2), otherwise mint a fresh UUID v4 from the OS
    /// entropy seam, shaped like the Home identity ids.
    fn remote_id_for(&self, canonical_url: &str) -> Result<String, ImportError> {
        if let Some(parent) = self.store.find_remote_parent_by_url(canonical_url)? {
            return Ok(parent.remote_id);
        }
        let mut bytes = [0u8; 16];
        if self.filesystem.read_entropy(&mut bytes).is_err() {
            let nanos = self.clock.unix_epoch_nanos() as u64;
            bytes[..8].copy_from_slice(&nanos.to_be_bytes());
            bytes[8..].copy_from_slice(&(nanos ^ 0x9e37_79b9_7f4a_7c15).to_be_bytes());
        }
        Ok(crate::seams::clock::uuid_v4_shape(&mut bytes))
    }

    fn discover_git_candidates(
        &self,
        spec: &GitSourceSpec,
        mirror: &Path,
        resolved: &ResolvedGitRef,
        force_full_depth: bool,
    ) -> Result<Vec<GitImportCandidate>, ImportError> {
        let entries = self.git_source.list_tree(mirror, &resolved.commit)?;
        let mut files = Vec::new();
        for entry in entries {
            if matches!(
                entry.kind,
                GitTreeEntryKind::Blob | GitTreeEntryKind::Symlink
            ) {
                files.push(entry.path);
            }
        }
        let mode = if force_full_depth {
            DiscoveryMode::ForceFullDepth
        } else {
            DiscoveryMode::StandardWithRecursiveFallback
        };
        let discovered = discover_skills_from_paths(
            &repo_name_from_url(&spec.url),
            &files,
            spec.path_prefix.as_deref(),
            mode,
        );
        let mut candidates = Vec::with_capacity(discovered.len());
        for skill in discovered {
            let identity_key = normalize_identity(&skill.directory_name)?;
            let document_path = skill_document_path(&skill.skill_path);
            let Ok(Some(blob)) = self.git_source.read_blob(
                mirror,
                &resolved.commit,
                &document_path,
                usize::try_from(MAX_SKILL_DOCUMENT_BYTES).expect("512 KiB fits usize"),
            ) else {
                // Unreadable or oversized SKILL.md: the candidate is not selectable.
                continue;
            };
            let metadata = parse_skill_metadata(&String::from_utf8_lossy(&blob));
            candidates.push(GitImportCandidate {
                directory_name: skill.directory_name.clone(),
                identity_key,
                display_name: metadata
                    .name
                    .clone()
                    .unwrap_or_else(|| skill.directory_name.clone()),
                description: metadata.description.unwrap_or_default(),
                frontmatter_name: metadata.name,
                skill_path: skill.skill_path,
            });
        }
        Ok(candidates)
    }

    fn next_git_operation_id(&self) -> String {
        format!(
            "git-import-{}-{}",
            self.clock.unix_epoch_nanos(),
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        )
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
        let (discovered, truncated) =
            discover_staged_skills(self.filesystem.as_ref(), &staged_source)?;
        if discovered.is_empty() {
            return self.cleanup_staging_after_error(
                staging_operation_root,
                None,
                ImportError::Validation(
                    "file Import source must contain a readable SKILL.md".into(),
                ),
            );
        }
        let staged_sources = discovered
            .into_iter()
            .filter(|(directory_name, _)| {
                selected_directory_names
                    .as_ref()
                    .is_none_or(|selected| selected.contains(directory_name))
            })
            .map(|(directory_name, staged_skill_path)| StagedFileSource {
                original_path: staged_source.original_path.clone(),
                original_filename: staged_source.original_filename.clone(),
                suggested_root_name: directory_name,
                staged_content_root: staged_skill_path,
            })
            .collect::<Vec<_>>();
        let (candidates, staging_fingerprint) =
            self.validate_prestaged_sources(staging_operation_root, staged_sources)?;
        Ok((candidates, staging_fingerprint, truncated))
    }

    /// Validate already-staged Skill roots (one per candidate) and produce the
    /// validated candidates. Cleans the staging root on any validation failure.
    fn validate_prestaged_sources(
        &self,
        staging_operation_root: &Path,
        staged_sources: Vec<StagedFileSource>,
    ) -> Result<(Vec<ValidatedFileCandidate>, DirectoryFingerprint), ImportError> {
        let staging_fingerprint = self
            .filesystem
            .directory_fingerprint(staging_operation_root)?;
        let validation = (|| {
            let mut candidates = Vec::with_capacity(staged_sources.len());
            for staged_source in staged_sources {
                let directory_name = staged_source.suggested_root_name.clone();
                let identity_key = normalize_identity(&directory_name)?;
                let tree_snapshot = self
                    .filesystem
                    .staged_tree_snapshot(&staged_source.staged_content_root)?;
                validate_staged_tree(self.filesystem.as_ref(), &tree_snapshot)?;
                let skill_markdown = self
                    .filesystem
                    .read_skill_document(&staged_source.staged_content_root)
                    .map_err(|error| {
                        ImportError::Validation(format!("Import SKILL.md is not readable: {error}"))
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
                candidates.push((candidate, staged_source, tree_snapshot));
            }
            Ok(candidates)
        })();
        match validation {
            Ok(candidates) => Ok((candidates, staging_fingerprint)),
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
        if self.write_gate.is_product_write_open() {
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

fn remote_record_from_plan(
    plan: &PlannedFileImport,
    remote: &PlannedRemoteImport,
    remote_id: String,
    original_commit_known: bool,
    provider_hash: Option<String>,
) -> RemoteImportRecord {
    let content_hash = plan.tree_snapshot.content_hash.clone();
    RemoteImportRecord {
        skill_id: plan.skill_id.clone(),
        directory_name: plan.candidate.directory_name.clone(),
        identity_key: plan.candidate.identity_key.clone(),
        display_name: plan.candidate.display_name.clone(),
        description: plan.candidate.description.clone(),
        library_entry_path: plan.final_entity_path.clone(),
        final_entity_path: plan.final_entity_path.clone(),
        recorded_content_hash: content_hash.clone(),
        remote_id,
        // The recorded URL is the canonical repository identity when it
        // normalizes; user-authored import URLs that cannot normalize
        // (http, shorthand) keep their exact spelling so the mirror path
        // and the fetch stay stable (spec §3.4 normalization applies to
        // lock-derived and migrated rows).
        source_url: crate::adapters::remote_provider::normalize_catalog_url(&remote.repo_url)
            .unwrap_or_else(|_| remote.repo_url.clone()),
        requested_ref: remote.requested_ref.clone(),
        verification_anchor_commit: remote.resolved_commit.clone(),
        original_commit_known,
        skill_path: remote.skill_path.clone(),
        provider_hash,
        remote_baseline_hash: content_hash.clone(),
        current_baseline_hash: content_hash,
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

pub(crate) fn validate_staged_tree(
    filesystem: &dyn FileSystem,
    snapshot: &StagedTreeSnapshot,
) -> Result<(), ImportError> {
    let mut paths = HashMap::new();
    let mut total_bytes = 0_u64;
    for entry in &snapshot.entries {
        if entry.relative_path.to_str().is_none() {
            return Err(ImportError::Validation(
                "Skill paths must be valid UTF-8".into(),
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
                        "Skill file exceeds its size limit: {}",
                        entry.relative_path.display()
                    )));
                }
                total_bytes = total_bytes.saturating_add(*length);
            }
            StagedEntryKind::Symlink { target } => {
                if target.to_str().is_none() || target.is_absolute() {
                    return Err(ImportError::Validation(format!(
                        "Skill symlink target must be a valid UTF-8 relative path: {}",
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
            "Skill must contain a readable SKILL.md".into(),
        ));
    }
    if total_bytes > MAX_SKILL_BYTES || snapshot.total_file_bytes > MAX_SKILL_BYTES {
        return Err(ImportError::Validation(
            "Skill exceeds the 256 MB size limit".into(),
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
                        "Skill symlink escapes the Skill root".into(),
                    ));
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(ImportError::Validation(
                    "Skill symlink target must be relative".into(),
                ));
            }
        }
    }
    Ok(segments.into_iter().collect())
}

fn unsafe_staged_symlink(link: &Path) -> ImportError {
    ImportError::Validation(format!(
        "Skill symlink escapes the Skill root, is dangling, or is cyclic: {}",
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

pub(crate) fn normalize_identity(directory_name: &str) -> Result<String, ImportError> {
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
    #[error("the installed Skill has local modifications; abandon them to Update")]
    Modified,
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

impl From<GitSourceParseError> for ImportError {
    fn from(error: GitSourceParseError) -> Self {
        ImportError::Validation(error.to_string())
    }
}

impl From<GitResolveError> for ImportError {
    fn from(error: GitResolveError) -> Self {
        match error {
            GitResolveError::RefNotFound(reference) => ImportError::Validation(format!(
                "the requested Git ref '{reference}' was not found in the repository"
            )),
            GitResolveError::Source(source) => ImportError::Source(source),
        }
    }
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
