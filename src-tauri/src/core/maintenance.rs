use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, Health, SkillId, SourceKind, parse_skill_metadata,
};
use crate::seams::activation_store::{ActivationObservation, ActivationStoreError};
use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileImportRecoveryBaseline, FileSystem,
    FileSystemError, LinkSourceSnapshot, RelocateActivationStep, RelocateInitialEntry,
    RelocateJournal, RelocateJournalPhase, RelocateRecoveryBaseline, RemoveActivationStep,
    RemoveInitialEntry, RemoveJournal, RemoveJournalPhase, RemoveRecoveryBaseline,
    RemoveSourceKind, SkillFingerprint,
};
use crate::seams::maintenance_store::{
    LinkSkillRecord, MaintenanceStore, MaintenanceStoreError, ManagedSkillBaseline,
    RelocateActivationBaseline, RemoveTarget, SkillHealthObservation,
};
use crate::seams::recovery::RecoveryGate;

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationHealthReport {
    pub checked: u32,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum MaintenanceError {
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Store(#[from] ActivationStoreError),
    #[error(transparent)]
    MaintenanceStore(#[from] MaintenanceStoreError),
    #[error("the Managed Skill was not found: {0}")]
    SkillNotFound(String),
    #[error("the Skill is not a Link and cannot be relocated: {0}")]
    NotLink(String),
    #[error("relocation validation failed: {0}")]
    Validation(String),
    #[error("the relocation plan no longer matches the filesystem")]
    PlanStale,
    #[error("the relocation plan was not found or has expired")]
    PlanNotFound,
    #[error("startup recovery is still in progress")]
    RecoveryInProgress,
    #[error(
        "relocation state failed and filesystem compensation also failed; recovery is required: state={state_error}; compensation={compensation_error}"
    )]
    RecoveryRequired {
        state_error: String,
        compensation_error: String,
    },
    #[error("internal Maintenance error: {0}")]
    Internal(String),
}

/// The new location a Broken Link points at, after validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocatePreview {
    pub plan_token: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub source_entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub display_name: String,
    pub description: String,
    pub frontmatter_name: Option<String>,
    pub activation_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelocateResult {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub final_entity_path: PathBuf,
    pub activation_count: u32,
    pub snapshot_version: u64,
}

#[derive(Clone)]
struct PlannedRelocate {
    skill: LinkSkillRecord,
    source_snapshot: LinkSourceSnapshot,
    candidate: RelocateCandidate,
    fingerprint: SkillFingerprint,
    activations: Vec<RelocateActivationBaseline>,
    created_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovePreview {
    pub plan_token: String,
    pub skill_id: SkillId,
    pub directory_name: String,
    pub source_kind: SourceKind,
    pub final_entity_path: PathBuf,
    pub activation_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveResult {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub snapshot_version: u64,
}

#[derive(Clone)]
struct PlannedRemove {
    target: RemoveTarget,
    activations: Vec<RelocateActivationBaseline>,
    /// Install entity fingerprint at plan time; `None` when the entity is
    /// already gone (Broken Install).
    entity_fingerprint: Option<DirectoryFingerprint>,
    created_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RelocateCandidate {
    directory_name: String,
    display_name: String,
    description: String,
    frontmatter_name: Option<String>,
    final_entity_path: PathBuf,
}

pub struct MaintenanceService {
    store: Arc<dyn MaintenanceStore>,
    filesystem: Arc<dyn FileSystem>,
    library_root: Option<PathBuf>,
    recovery_gate: Arc<RecoveryGate>,
    relocate_plans: Arc<Mutex<HashMap<String, PlannedRelocate>>>,
    remove_plans: Arc<Mutex<HashMap<String, PlannedRemove>>>,
    next_plan_id: Arc<AtomicU64>,
    plan_ttl: Duration,
}

impl Clone for MaintenanceService {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            filesystem: self.filesystem.clone(),
            library_root: self.library_root.clone(),
            recovery_gate: self.recovery_gate.clone(),
            relocate_plans: self.relocate_plans.clone(),
            remove_plans: self.remove_plans.clone(),
            next_plan_id: self.next_plan_id.clone(),
            plan_ttl: self.plan_ttl,
        }
    }
}

impl MaintenanceService {
    pub fn new(store: Arc<dyn MaintenanceStore>, filesystem: Arc<dyn FileSystem>) -> Self {
        Self {
            store,
            filesystem,
            library_root: None,
            recovery_gate: Arc::new(RecoveryGate::ready()),
            relocate_plans: Arc::new(Mutex::new(HashMap::new())),
            remove_plans: Arc::new(Mutex::new(HashMap::new())),
            next_plan_id: Arc::new(AtomicU64::new(1)),
            plan_ttl: DEFAULT_PLAN_TTL,
        }
    }

    pub fn with_library_root(mut self, library_root: PathBuf) -> Self {
        self.library_root = Some(library_root);
        self
    }

    pub fn with_recovery_gate(mut self, recovery_gate: Arc<RecoveryGate>) -> Self {
        self.recovery_gate = recovery_gate;
        self
    }

    pub fn with_plan_ttl(mut self, plan_ttl: Duration) -> Self {
        self.plan_ttl = plan_ttl;
        self
    }

    pub fn begin_startup(self) -> StartupMaintenance {
        let worker_service = self.clone();
        let worker = std::thread::spawn(move || worker_service.startup_check());
        StartupMaintenance {
            maintenance: self,
            startup_worker: Mutex::new(Some(worker)),
        }
    }

    pub fn startup_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        self.recover_startup_operations()?;
        self.recovery_gate.mark_ready();
        self.run_health_check()
    }

    fn recover_startup_operations(&self) -> Result<(), MaintenanceError> {
        if let Some(library_root) = &self.library_root {
            let baselines = self
                .store
                .installed_skill_baselines()?
                .into_iter()
                .map(|baseline| FileImportRecoveryBaseline {
                    skill_id: baseline.skill_id.0,
                    final_entity_path: baseline.final_entity_path,
                    recorded_content_hash: baseline.recorded_content_hash,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_file_import_journals(library_root, &baselines)?;
            let entities = self
                .store
                .adopted_skill_entities()?
                .into_iter()
                .map(|entity| FileImportRecoveryBaseline {
                    skill_id: entity.skill_id.0,
                    final_entity_path: entity.final_entity_path,
                    recorded_content_hash: entity.recorded_content_hash.unwrap_or_default(),
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_adopt_journals(library_root, &baselines, &entities)?;
            let desired_activations = self.store.desired_activation_baselines()?;
            self.filesystem
                .recover_activation_replace_journals(library_root, &desired_activations)?;
            let relocate_baselines = self
                .store
                .managed_skill_baselines()?
                .into_iter()
                .map(|baseline| RelocateRecoveryBaseline {
                    skill_id: baseline.skill_id.0,
                    final_entity_path: baseline.final_entity_path,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_relocate_journals(library_root, &relocate_baselines)?;
            let remove_baselines = self
                .store
                .managed_skill_baselines()?
                .into_iter()
                .map(|baseline| RemoveRecoveryBaseline {
                    skill_id: baseline.skill_id.0,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_remove_journals(library_root, &remove_baselines)?;
        }
        Ok(())
    }

    fn run_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let desired = self.store.desired_activations()?;
        let observations = desired
            .iter()
            .map(|activation| {
                let observed_state = match self
                    .filesystem
                    .activation_snapshot(&activation.expected_entry_path)?
                {
                    ActivationEntrySnapshot::Missing => ActivationObservedState::Missing,
                    ActivationEntrySnapshot::Other => ActivationObservedState::Occupied,
                    ActivationEntrySnapshot::Symlink { target }
                        if target != activation.expected_target_path =>
                    {
                        ActivationObservedState::TargetMismatch
                    }
                    ActivationEntrySnapshot::Symlink { .. } => {
                        if self
                            .filesystem
                            .skill_directory_is_readable(&activation.expected_target_path)?
                        {
                            ActivationObservedState::Present
                        } else {
                            ActivationObservedState::Dangling
                        }
                    }
                };
                Ok(ActivationObservation {
                    skill_id: activation.skill_id.clone(),
                    agent_id: activation.agent_id.clone(),
                    observed_state,
                })
            })
            .collect::<Result<Vec<_>, MaintenanceError>>()?;
        let mut snapshot_version = self.store.record_observations(&observations)?;
        let managed = self.store.managed_skill_baselines()?;
        if !managed.is_empty() {
            let health_observations = managed
                .into_iter()
                .map(|skill| {
                    let health = self.observe_skill_health(&skill)?;
                    Ok(SkillHealthObservation {
                        skill_id: skill.skill_id,
                        health,
                    })
                })
                .collect::<Result<Vec<_>, MaintenanceError>>()?;
            snapshot_version = self.store.record_skill_health(&health_observations)?;
        }
        Ok(ActivationHealthReport {
            checked: u32::try_from(observations.len()).map_err(|_| {
                MaintenanceError::Internal("Activation health result exceeds u32 range".into())
            })?,
            snapshot_version,
        })
    }

    /// Classify one Managed Skill from the filesystem alone. Install
    /// entities compare their tree hash against the recorded baseline:
    /// readable-and-identical is Healthy, readable-but-different is Modified,
    /// and anything unreadable (missing entity, missing SKILL.md, unreadable
    /// content) is Broken — never Modified. Links are readable-or-Broken;
    /// a Link whose entity is unreadable is Broken regardless of any hash.
    fn observe_skill_health(
        &self,
        skill: &ManagedSkillBaseline,
    ) -> Result<Health, MaintenanceError> {
        let readable = self
            .filesystem
            .skill_directory_is_readable(&skill.final_entity_path)?;
        if !readable {
            return Ok(Health::Broken);
        }
        if skill.source_kind == SourceKind::Link {
            return Ok(Health::Healthy);
        }
        let Some(recorded) = &skill.recorded_content_hash else {
            // No recorded baseline: the entity is present and readable, so
            // there is nothing to be Modified against.
            return Ok(Health::Healthy);
        };
        match self.filesystem.tree_hash(&skill.final_entity_path) {
            Ok(current) => Ok(if &current == recorded {
                Health::Healthy
            } else {
                Health::Modified
            }),
            Err(_) => {
                // §5.5: a failed read never yields a partial hash; treat an
                // unhashable entity as Broken rather than Modified.
                Ok(Health::Broken)
            }
        }
    }

    /// Relocate a Broken Link: validate the new source (SKILL.md readable,
    /// directory name identical, frontmatter name matching when the recorded
    /// display name came from frontmatter), then preview the pointer and
    /// Activation updates. Nothing is written.
    pub fn relocate(
        &self,
        skill_id: &SkillId,
        source_path: &Path,
    ) -> Result<RelocatePreview, MaintenanceError> {
        self.ensure_writes_ready()?;
        let Some(skill) = self.store.link_skill(skill_id)? else {
            return Err(MaintenanceError::NotLink(skill_id.0.clone()));
        };
        let (candidate, source_snapshot) = self.validate_relocate_source(&skill, source_path)?;
        let fingerprint = self
            .filesystem
            .skill_fingerprint(&candidate.final_entity_path)?;
        let activations = self.store.activation_baselines_for_skill(skill_id)?;
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("relocate-plan-{plan_number}");
        let mut plans = self
            .relocate_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("relocation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        plans.insert(
            plan_token.clone(),
            PlannedRelocate {
                skill: skill.clone(),
                source_snapshot,
                candidate: candidate.clone(),
                fingerprint,
                activations: activations.clone(),
                created_at: Instant::now(),
            },
        );
        Ok(RelocatePreview {
            plan_token,
            skill_id: skill_id.clone(),
            directory_name: skill.directory_name,
            source_entry_path: source_path.to_path_buf(),
            final_entity_path: candidate.final_entity_path,
            display_name: candidate.display_name,
            description: candidate.description,
            frontmatter_name: candidate.frontmatter_name,
            activation_count: u32::try_from(activations.len()).map_err(|_| {
                MaintenanceError::Internal("relocation Activation count exceeds u32".into())
            })?,
        })
    }

    /// Apply a planned relocation: re-preflight the new source and every
    /// Activation entry, then rewrite each symlink (journaled), commit the
    /// new pointer, and archive the journal. A crash between steps is rolled
    /// forward or back at next startup; a failed step compensates in reverse
    /// and, if compensation itself fails, leaves the journal for recovery.
    pub fn apply_relocate(&self, plan_token: &str) -> Result<RelocateResult, MaintenanceError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .relocate_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("relocation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        let plan = plans
            .remove(plan_token)
            .ok_or(MaintenanceError::PlanNotFound)?;
        drop(plans);

        // Re-preflight: the Link pointer, the new source and every Activation
        // entry must still match the plan.
        let current_skill = self
            .store
            .link_skill(&plan.skill.skill_id)?
            .ok_or(MaintenanceError::PlanStale)?;
        if current_skill != plan.skill {
            return Err(MaintenanceError::PlanStale);
        }
        let current_source = self
            .filesystem
            .inspect_link_source(&plan.source_snapshot.entry_path)
            .map_err(|_| MaintenanceError::PlanStale)?;
        if current_source != plan.source_snapshot {
            return Err(MaintenanceError::PlanStale);
        }
        let current = self
            .validate_relocate_source(&plan.skill, &plan.source_snapshot.entry_path)
            .map_err(|_| MaintenanceError::PlanStale)?
            .0;
        if current != plan.candidate {
            return Err(MaintenanceError::PlanStale);
        }
        let current_fingerprint = self
            .filesystem
            .skill_fingerprint(&current.final_entity_path)
            .map_err(|_| MaintenanceError::PlanStale)?;
        if current_fingerprint != plan.fingerprint {
            return Err(MaintenanceError::PlanStale);
        }
        let current_activations = self
            .store
            .activation_baselines_for_skill(&plan.skill.skill_id)?;
        if current_activations != plan.activations {
            return Err(MaintenanceError::PlanStale);
        }

        let library_root = self.library_root.clone().ok_or_else(|| {
            MaintenanceError::Internal("Maintenance library root is not configured".into())
        })?;
        let operation_id = format!(
            "relocate-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            plan.skill.skill_id.0
        );
        let steps = plan
            .activations
            .iter()
            .map(|activation| {
                let old_target = activation.expected_target_path.clone();
                let new_target = plan.candidate.final_entity_path.clone();
                let initial_entry = match self
                    .filesystem
                    .activation_snapshot(&activation.expected_entry_path)
                {
                    Ok(ActivationEntrySnapshot::Missing) => RelocateInitialEntry::Missing,
                    Ok(ActivationEntrySnapshot::Symlink { target }) if target == old_target => {
                        RelocateInitialEntry::Symlink {
                            old_target: old_target.clone(),
                        }
                    }
                    // A previous interrupted relocation may already have
                    // repointed the entry; treat it as already-applied.
                    Ok(ActivationEntrySnapshot::Symlink { target }) if target == new_target => {
                        RelocateInitialEntry::Symlink {
                            old_target: old_target.clone(),
                        }
                    }
                    _ => return Err(MaintenanceError::PlanStale),
                };
                Ok(RelocateActivationStep {
                    agent_id: activation.agent_id.0.clone(),
                    entry_path: activation.expected_entry_path.clone(),
                    old_target_path: old_target,
                    new_target_path: new_target,
                    initial_entry,
                })
            })
            .collect::<Result<Vec<_>, MaintenanceError>>()?;
        let mut journal = RelocateJournal {
            version: 1,
            operation_id: operation_id.clone(),
            phase: RelocateJournalPhase::Applying,
            skill_id: plan.skill.skill_id.0.clone(),
            old_final_entity_path: plan.skill.final_entity_path.clone(),
            new_final_entity_path: plan.candidate.final_entity_path.clone(),
            activations: steps,
        };
        self.filesystem
            .write_relocate_journal(&library_root, &journal)?;

        // Rewrite every Activation entry; on failure compensate in reverse.
        for index in 0..journal.activations.len() {
            let step = &mut journal.activations[index];
            let entry = match self.filesystem.activation_snapshot(&step.entry_path) {
                Ok(entry) => entry,
                Err(error) => {
                    return self.fail_relocate(&library_root, &journal, error.into());
                }
            };
            let outcome = match entry {
                ActivationEntrySnapshot::Missing => self
                    .filesystem
                    .create_activation(&step.new_target_path, &step.entry_path),
                ActivationEntrySnapshot::Symlink { target } if target == step.old_target_path => {
                    self.filesystem
                        .remove_activation(&step.entry_path)
                        .and_then(|_| {
                            self.filesystem
                                .create_activation(&step.new_target_path, &step.entry_path)
                        })
                }
                ActivationEntrySnapshot::Symlink { target } if target == step.new_target_path => {
                    // Already repointed (interrupted earlier relocation).
                    Ok(())
                }
                _ => Err(FileSystemError::PlanStale {
                    path: step.entry_path.clone(),
                }),
            };
            if let Err(error) = outcome {
                return self.fail_relocate(&library_root, &journal, error.into());
            }
            if let Err(error) = self
                .filesystem
                .write_relocate_journal(&library_root, &journal)
            {
                return self.fail_relocate(&library_root, &journal, error.into());
            }
        }

        // Commit the new pointer; every Activation's expected target and the
        // Skill's final entity move together in one transaction.
        let snapshot_version = match self.store.commit_relocate(
            &plan.skill.skill_id,
            plan.candidate.final_entity_path.clone(),
            plan.candidate.display_name.clone(),
            plan.candidate.description.clone(),
            plan.candidate.final_entity_path.clone(),
            &plan.activations,
        ) {
            Ok(version) => version,
            Err(error) => {
                return self.fail_relocate(&library_root, &journal, error.into());
            }
        };
        journal.phase = RelocateJournalPhase::Committed;
        if let Err(error) = self
            .filesystem
            .write_relocate_journal(&library_root, &journal)
        {
            return Err(MaintenanceError::RecoveryRequired {
                state_error: "the relocation catalog write committed".into(),
                compensation_error: format!(
                    "the committed journal could not be persisted: {error}"
                ),
            });
        }
        if let Err(error) = self
            .filesystem
            .finish_relocate_journal(&library_root, &operation_id)
        {
            return Err(MaintenanceError::RecoveryRequired {
                state_error: "the relocation catalog write committed".into(),
                compensation_error: format!("the completed journal could not be archived: {error}"),
            });
        }
        let activation_count = u32::try_from(journal.activations.len()).map_err(|_| {
            MaintenanceError::Internal("relocation Activation count exceeds u32".into())
        })?;
        Ok(RelocateResult {
            skill_id: plan.skill.skill_id,
            directory_name: plan.skill.directory_name,
            final_entity_path: plan.candidate.final_entity_path,
            activation_count,
            snapshot_version,
        })
    }

    pub fn cancel_relocate(&self, plan_token: &str) -> Result<bool, MaintenanceError> {
        let mut plans = self
            .relocate_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("relocation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        Ok(plans.remove(plan_token).is_some())
    }

    /// Validate a new Link source against the recorded pointer: it must live
    /// outside the Library, expose a readable SKILL.md, keep the same
    /// directory identity, and — when the recorded display name came from
    /// frontmatter — keep the same frontmatter name. Returns the validated
    /// candidate together with the source snapshot the plan re-checks at
    /// Apply time.
    fn validate_relocate_source(
        &self,
        skill: &LinkSkillRecord,
        source_path: &Path,
    ) -> Result<(RelocateCandidate, LinkSourceSnapshot), MaintenanceError> {
        let source_snapshot = self.filesystem.inspect_link_source(source_path)?;
        let final_entity_path = source_snapshot.final_entity_path.clone();
        if let Some(library_root) = &self.library_root {
            let canonical_library = self.filesystem.normalize_configured_path(library_root)?;
            if source_snapshot.entry_path.starts_with(&canonical_library)
                || final_entity_path.starts_with(&canonical_library)
            {
                return Err(MaintenanceError::Validation(
                    "relocated Link sources must remain outside the Library".into(),
                ));
            }
        }
        if !self
            .filesystem
            .skill_directory_is_readable(&final_entity_path)?
        {
            return Err(MaintenanceError::Validation(format!(
                "the new source is not a readable Skill: {}",
                final_entity_path.display()
            )));
        }
        let directory_name = source_snapshot.directory_name.clone();
        if directory_name != skill.directory_name {
            return Err(MaintenanceError::Validation(format!(
                "the relocated directory '{}' must keep the identity '{}'",
                directory_name, skill.directory_name
            )));
        }
        let skill_markdown = self.filesystem.read_skill_document(&final_entity_path)?;
        let metadata = parse_skill_metadata(&skill_markdown);
        let frontmatter_name = metadata.name.clone();
        if let Some(name) = &frontmatter_name {
            let recorded_name = if skill.display_name != skill.directory_name {
                Some(skill.display_name.as_str())
            } else {
                None
            };
            let matches = recorded_name.map_or_else(
                || name == &skill.directory_name,
                |recorded| name == recorded,
            );
            if !matches {
                return Err(MaintenanceError::Validation(format!(
                    "the relocated frontmatter name '{name}' does not match the recorded name"
                )));
            }
        }
        let display_name = metadata
            .name
            .clone()
            .unwrap_or_else(|| directory_name.clone());
        Ok((
            RelocateCandidate {
                directory_name,
                display_name,
                description: metadata.description.unwrap_or_default(),
                frontmatter_name,
                final_entity_path,
            },
            source_snapshot,
        ))
    }

    /// Compensate a failed relocation in reverse: entries already pointing
    /// at the new entity are returned to their planned initial state. A
    /// successful compensation archives the journal and returns the original
    /// error; a failed one leaves the journal for startup recovery.
    fn fail_relocate(
        &self,
        library_root: &Path,
        journal: &RelocateJournal,
        original: MaintenanceError,
    ) -> Result<RelocateResult, MaintenanceError> {
        let mut compensation_errors = Vec::new();
        for step in journal.activations.iter().rev() {
            let entry = self.filesystem.activation_snapshot(&step.entry_path);
            let outcome = match entry {
                Ok(ActivationEntrySnapshot::Symlink { target })
                    if target == step.new_target_path =>
                {
                    match &step.initial_entry {
                        RelocateInitialEntry::Missing => {
                            self.filesystem.remove_activation(&step.entry_path)
                        }
                        RelocateInitialEntry::Symlink { old_target } => self
                            .filesystem
                            .remove_activation(&step.entry_path)
                            .and_then(|_| {
                                self.filesystem
                                    .create_activation(old_target, &step.entry_path)
                            }),
                    }
                }
                // Missing after a partial remove, or the original symlink:
                // restore the original state.
                Ok(ActivationEntrySnapshot::Missing) => match &step.initial_entry {
                    RelocateInitialEntry::Missing => Ok(()),
                    RelocateInitialEntry::Symlink { old_target } => self
                        .filesystem
                        .create_activation(old_target, &step.entry_path),
                },
                Ok(ActivationEntrySnapshot::Symlink { target })
                    if target == step.old_target_path =>
                {
                    Ok(())
                }
                _ => Err(FileSystemError::PlanStale {
                    path: step.entry_path.clone(),
                }),
            };
            if let Err(error) = outcome {
                compensation_errors.push(error.to_string());
            }
        }
        if !compensation_errors.is_empty() {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: compensation_errors.join("; "),
            });
        }
        if let Err(error) = self
            .filesystem
            .finish_relocate_journal(library_root, &journal.operation_id)
        {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: format!(
                    "compensation succeeded but the journal could not be archived: {error}"
                ),
            });
        }
        Err(original)
    }

    /// Plan a Remove (§8.6): every desired Activation entry must currently
    /// be absent or a symlink to its recorded target, so the apply can
    /// verify before deleting. Links keep their external entity; Installs
    /// are fingerprinted so apply re-preflights the entity (a Broken
    /// Install has no entity and is fingerprinted as absent).
    pub fn plan_remove(&self, skill_id: &SkillId) -> Result<RemovePreview, MaintenanceError> {
        self.ensure_writes_ready()?;
        let Some(target) = self.store.remove_target(skill_id)? else {
            return Err(MaintenanceError::SkillNotFound(skill_id.0.clone()));
        };
        let activations = self.store.activation_baselines_for_skill(skill_id)?;
        for activation in &activations {
            match self
                .filesystem
                .activation_snapshot(&activation.expected_entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target }
                    if target == activation.expected_target_path => {}
                ActivationEntrySnapshot::Symlink { .. } => {
                    return Err(MaintenanceError::Validation(format!(
                        "the Activation entry '{}' no longer matches its recorded target",
                        activation.expected_entry_path.display()
                    )));
                }
                ActivationEntrySnapshot::Other => {
                    return Err(MaintenanceError::Validation(format!(
                        "the Activation entry '{}' is occupied by external content",
                        activation.expected_entry_path.display()
                    )));
                }
            }
        }
        let entity_fingerprint = if target.source_kind.is_install() {
            self.filesystem
                .directory_fingerprint(&target.final_entity_path)
                .ok()
        } else {
            None
        };
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("remove-plan-{plan_number}");
        let mut plans = self
            .remove_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("Remove plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        plans.insert(
            plan_token.clone(),
            PlannedRemove {
                target: target.clone(),
                activations: activations.clone(),
                entity_fingerprint,
                created_at: Instant::now(),
            },
        );
        Ok(RemovePreview {
            plan_token,
            skill_id: skill_id.clone(),
            directory_name: target.directory_name,
            source_kind: target.source_kind,
            final_entity_path: target.final_entity_path,
            activation_count: u32::try_from(activations.len()).map_err(|_| {
                MaintenanceError::Internal("Remove Activation count exceeds u32".into())
            })?,
        })
    }

    /// Apply a planned Remove (§8.6): disable every Activation first, back
    /// up an Install entity, then delete the catalog row (activations and
    /// source tables cascade; the journal archives the audit). A failure
    /// compensates in reverse — restore the entity, recreate the removed
    /// symlinks — and a failed compensation locks writes for recovery.
    pub fn apply_remove(&self, plan_token: &str) -> Result<RemoveResult, MaintenanceError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .remove_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("Remove plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        let plan = plans
            .remove(plan_token)
            .ok_or(MaintenanceError::PlanNotFound)?;
        drop(plans);

        // Re-preflight: the row, every Activation entry and the Install
        // entity must still match the plan.
        let current_target = self
            .store
            .remove_target(&plan.target.skill_id)?
            .ok_or(MaintenanceError::PlanStale)?;
        if current_target != plan.target {
            return Err(MaintenanceError::PlanStale);
        }
        let current_activations = self
            .store
            .activation_baselines_for_skill(&plan.target.skill_id)?;
        if current_activations != plan.activations {
            return Err(MaintenanceError::PlanStale);
        }
        for activation in &plan.activations {
            match self
                .filesystem
                .activation_snapshot(&activation.expected_entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target }
                    if target == activation.expected_target_path => {}
                _ => return Err(MaintenanceError::PlanStale),
            }
        }
        if plan.target.source_kind.is_install() {
            match (
                &plan.entity_fingerprint,
                self.filesystem
                    .directory_fingerprint(&plan.target.final_entity_path),
            ) {
                (Some(expected), Ok(current)) if &current == expected => {}
                (None, Err(_)) => {}
                _ => return Err(MaintenanceError::PlanStale),
            }
        }

        let library_root = self.library_root.clone().ok_or_else(|| {
            MaintenanceError::Internal("Maintenance library root is not configured".into())
        })?;
        let operation_id = format!(
            "remove-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
            plan.target.skill_id.0
        );
        let source_kind = if plan.target.source_kind == SourceKind::Link {
            RemoveSourceKind::Link
        } else {
            RemoveSourceKind::Install
        };
        let steps = plan
            .activations
            .iter()
            .map(|activation| {
                let initial_entry = self
                    .filesystem
                    .activation_snapshot(&activation.expected_entry_path)
                    .map_err(|_| MaintenanceError::PlanStale)?;
                Ok(RemoveActivationStep {
                    agent_id: activation.agent_id.0.clone(),
                    entry_path: activation.expected_entry_path.clone(),
                    target_path: activation.expected_target_path.clone(),
                    initial_entry: if initial_entry == ActivationEntrySnapshot::Missing {
                        RemoveInitialEntry::Missing
                    } else {
                        RemoveInitialEntry::Symlink
                    },
                })
            })
            .collect::<Result<Vec<_>, MaintenanceError>>()?;
        let mut journal = RemoveJournal {
            version: 1,
            operation_id: operation_id.clone(),
            phase: RemoveJournalPhase::Applying,
            skill_id: plan.target.skill_id.0.clone(),
            source_kind,
            final_entity_path: plan.target.final_entity_path.clone(),
            backup_path: None,
            backup_fingerprint: None,
            activations: steps,
        };
        self.filesystem
            .write_remove_journal(&library_root, &journal)?;

        // Step 1: disable every Activation (§6.2 — verify, then remove; never
        // delete anything that is not the recorded symlink).
        for step in &journal.activations {
            let outcome = match self.filesystem.activation_snapshot(&step.entry_path) {
                Ok(ActivationEntrySnapshot::Missing) => Ok(()),
                Ok(ActivationEntrySnapshot::Symlink { target }) if target == step.target_path => {
                    self.filesystem.remove_activation(&step.entry_path)
                }
                _ => Err(FileSystemError::PlanStale {
                    path: step.entry_path.clone(),
                }),
            };
            if let Err(error) = outcome {
                return self.fail_remove(&library_root, &journal, error.into());
            }
        }

        // Step 2: Install entities move to the operation backup before the
        // catalog commit; Link entities stay at their external source.
        if source_kind == RemoveSourceKind::Install {
            let backup_path = library_root
                .join("operations")
                .join(&operation_id)
                .join("backup")
                .join(&plan.target.directory_name);
            match self.filesystem.backup_library_entity(
                &journal.final_entity_path,
                &backup_path,
                &library_root,
            ) {
                Ok(fingerprint) => {
                    journal.backup_path = Some(backup_path);
                    journal.backup_fingerprint = Some(fingerprint);
                }
                Err(FileSystemError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    // A Broken Install's entity is already gone; the catalog
                    // row is the only thing left to delete.
                }
                Err(error) => return self.fail_remove(&library_root, &journal, error.into()),
            }
            if let Err(error) = self
                .filesystem
                .write_remove_journal(&library_root, &journal)
            {
                return self.fail_remove(&library_root, &journal, error.into());
            }
        }

        // Step 3: catalog delete; activations, file_sources and
        // remote_sources cascade with the row (§5.4).
        let snapshot_version = match self.store.delete_skill(&plan.target.skill_id) {
            Ok(version) => version,
            Err(error) => return self.fail_remove(&library_root, &journal, error.into()),
        };
        journal.phase = RemoveJournalPhase::Committed;
        if let Err(error) = self
            .filesystem
            .write_remove_journal(&library_root, &journal)
        {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: "the Remove catalog delete committed".into(),
                compensation_error: format!(
                    "the committed journal could not be persisted: {error}"
                ),
            });
        }

        // Step 4: discard the backup and archive the journal audit.
        if let (Some(backup_path), Some(fingerprint)) =
            (&journal.backup_path, &journal.backup_fingerprint)
        {
            if let Err(error) = self.filesystem.discard_library_entity_backup(
                backup_path,
                &library_root,
                Some(fingerprint),
            ) {
                self.recovery_gate.mark_blocked();
                return Err(MaintenanceError::RecoveryRequired {
                    state_error: "the Remove catalog delete committed".into(),
                    compensation_error: format!(
                        "the entity backup could not be discarded: {error}"
                    ),
                });
            }
        }
        if let Err(error) = self
            .filesystem
            .finish_remove_journal(&library_root, &operation_id)
        {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: "the Remove catalog delete committed".into(),
                compensation_error: format!("the completed journal could not be archived: {error}"),
            });
        }
        Ok(RemoveResult {
            skill_id: plan.target.skill_id,
            directory_name: plan.target.directory_name,
            snapshot_version,
        })
    }

    pub fn cancel_remove(&self, plan_token: &str) -> Result<bool, MaintenanceError> {
        let mut plans = self
            .remove_plans
            .lock()
            .map_err(|_| MaintenanceError::Internal("Remove plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        Ok(plans.remove(plan_token).is_some())
    }

    /// Compensate a failed Remove in reverse: restore the backed-up entity,
    /// then recreate the removed Activation symlinks (only entries that were
    /// symlinks at plan time). Success archives the journal and returns the
    /// original error; failure locks writes for startup recovery.
    fn fail_remove(
        &self,
        library_root: &Path,
        journal: &RemoveJournal,
        original: MaintenanceError,
    ) -> Result<RemoveResult, MaintenanceError> {
        let mut compensation_errors = Vec::new();
        if let (Some(backup_path), Some(fingerprint)) =
            (&journal.backup_path, &journal.backup_fingerprint)
        {
            if let Err(error) = self.filesystem.restore_library_entity(
                backup_path,
                &journal.final_entity_path,
                library_root,
                fingerprint,
            ) {
                compensation_errors.push(error.to_string());
            }
        }
        for step in journal.activations.iter().rev() {
            let outcome = match self.filesystem.activation_snapshot(&step.entry_path) {
                Ok(ActivationEntrySnapshot::Symlink { target }) if target == step.target_path => {
                    Ok(())
                }
                Ok(ActivationEntrySnapshot::Missing)
                    if step.initial_entry == RemoveInitialEntry::Symlink =>
                {
                    self.filesystem
                        .create_activation(&step.target_path, &step.entry_path)
                }
                Ok(ActivationEntrySnapshot::Missing) => Ok(()),
                _ => Err(FileSystemError::PlanStale {
                    path: step.entry_path.clone(),
                }),
            };
            if let Err(error) = outcome {
                compensation_errors.push(error.to_string());
            }
        }
        if !compensation_errors.is_empty() {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: compensation_errors.join("; "),
            });
        }
        if let Err(error) = self
            .filesystem
            .finish_remove_journal(library_root, &journal.operation_id)
        {
            self.recovery_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: format!(
                    "compensation succeeded but the journal could not be archived: {error}"
                ),
            });
        }
        Err(original)
    }

    fn ensure_writes_ready(&self) -> Result<(), MaintenanceError> {
        if self.recovery_gate.writes_are_ready() {
            Ok(())
        } else {
            Err(MaintenanceError::RecoveryInProgress)
        }
    }
}

pub struct StartupMaintenance {
    maintenance: MaintenanceService,
    startup_worker: Mutex<Option<JoinHandle<Result<ActivationHealthReport, MaintenanceError>>>>,
}

impl StartupMaintenance {
    pub fn run_activation_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let mut startup_worker = self
            .startup_worker
            .lock()
            .map_err(|_| MaintenanceError::Internal("startup Maintenance lock poisoned".into()))?;
        if let Some(worker) = startup_worker.take() {
            worker.join().unwrap_or_else(|_| {
                Err(MaintenanceError::Internal(
                    "startup Maintenance task panicked".into(),
                ))
            })?;
        }
        drop(startup_worker);
        // §10.4: a retry after a failed startup recovery must re-run the
        // journal recovery itself, not just a read-only scan — otherwise the
        // lock notice clears while writes stay refused.
        if self.maintenance.recovery_gate.writes_are_ready() {
            self.maintenance.run_health_check()
        } else {
            self.maintenance.startup_check()
        }
    }

    pub fn relocate(
        &self,
        skill_id: &SkillId,
        source_path: &Path,
    ) -> Result<RelocatePreview, MaintenanceError> {
        self.maintenance.relocate(skill_id, source_path)
    }

    pub fn apply_relocate(&self, plan_token: &str) -> Result<RelocateResult, MaintenanceError> {
        self.maintenance.apply_relocate(plan_token)
    }

    pub fn cancel_relocate(&self, plan_token: &str) -> Result<bool, MaintenanceError> {
        self.maintenance.cancel_relocate(plan_token)
    }

    pub fn plan_remove(&self, skill_id: &SkillId) -> Result<RemovePreview, MaintenanceError> {
        self.maintenance.plan_remove(skill_id)
    }

    pub fn apply_remove(&self, plan_token: &str) -> Result<RemoveResult, MaintenanceError> {
        self.maintenance.apply_remove(plan_token)
    }

    pub fn cancel_remove(&self, plan_token: &str) -> Result<bool, MaintenanceError> {
        self.maintenance.cancel_remove(plan_token)
    }
}
