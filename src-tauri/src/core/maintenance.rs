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
use crate::core::source_lifecycle::SourceLifecycleService;
use crate::core::source_transition::{SourceTransitionError, SourceTransitionService};
use crate::core::source_update::SourceUpdateService;
use crate::core::write_gate::{
    HomeWriteContext, PlanCheck, PlanTicket, ProductWriteGuard, WriteGate, WriteGateError,
    WriteGateState,
};
use crate::seams::activation_store::{ActivationObservation, ActivationStoreError};
use crate::seams::filesystem::ActivationRecoveryBaseline;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileImportRecoveryBaseline, FileSystem,
    FileSystemError, HandoffItemPhase, HandoffJournal, HandoffJournalItem, LinkSourceSnapshot,
    RelocateActivationStep, RelocateInitialEntry, RelocateJournal, RelocateJournalPhase,
    RelocateRecoveryBaseline, RemoteParentManifest, RemoveActivationStep, RemoveInitialEntry,
    RemoveJournal, RemoveJournalPhase, RemoveRecoveryBaseline, RemoveSourceKind, SkillFingerprint,
};
use crate::seams::import_store::RemoteImportRecord;
use crate::seams::maintenance_store::{
    HandoffRecoveredRecord, LinkSkillRecord, MaintenanceStore, MaintenanceStoreError,
    ManagedSkillBaseline, RelocateActivationBaseline, RemoveTarget, SkillHealthObservation,
};
use crate::seams::scan_evidence_store::ScanEvidenceStoreFactory;

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
    #[error(transparent)]
    SourceTransition(#[from] SourceTransitionError),
    #[error(transparent)]
    SourceLifecycle(#[from] crate::core::source_lifecycle::SourceLifecycleError),
    #[error(transparent)]
    SourceUpdate(#[from] crate::core::source_update::SourceUpdateError),
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
    write_context: HomeWriteContext,
    gate_generation: u64,
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
    /// already absent (Broken Install).
    entity_fingerprint: Option<DirectoryFingerprint>,
    write_context: HomeWriteContext,
    gate_generation: u64,
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
    configured_library_root: Option<PathBuf>,
    /// Present only in the production composition, where Home paths must
    /// follow the bootstrap-verified identity across transitions.
    home_context: Option<Arc<WriteGate>>,
    write_gate: Arc<WriteGate>,
    relocate_plans: Arc<Mutex<HashMap<String, PlannedRelocate>>>,
    remove_plans: Arc<Mutex<HashMap<String, PlannedRemove>>>,
    next_plan_id: Arc<AtomicU64>,
    plan_ttl: Duration,
    startup_recovery: StartupRecovery,
}

#[derive(Clone)]
pub struct StartupRecoveryServices {
    source_transition: Arc<SourceTransitionService>,
    source_update: Arc<SourceUpdateService>,
    source_lifecycle: Arc<SourceLifecycleService>,
    scan_evidence_factory: Arc<dyn ScanEvidenceStoreFactory>,
}

impl StartupRecoveryServices {
    pub fn new(
        source_transition: Arc<SourceTransitionService>,
        source_update: Arc<SourceUpdateService>,
        source_lifecycle: Arc<SourceLifecycleService>,
        scan_evidence_factory: Arc<dyn ScanEvidenceStoreFactory>,
    ) -> Self {
        Self {
            source_transition,
            source_update,
            source_lifecycle,
            scan_evidence_factory,
        }
    }
}

#[derive(Clone)]
enum StartupRecovery {
    Configured(StartupRecoveryServices),
    TestOnly,
}

impl Clone for MaintenanceService {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            filesystem: self.filesystem.clone(),
            configured_library_root: self.configured_library_root.clone(),
            home_context: self.home_context.clone(),
            write_gate: self.write_gate.clone(),
            relocate_plans: self.relocate_plans.clone(),
            remove_plans: self.remove_plans.clone(),
            next_plan_id: self.next_plan_id.clone(),
            plan_ttl: self.plan_ttl,
            startup_recovery: self.startup_recovery.clone(),
        }
    }
}

impl MaintenanceService {
    pub fn new(
        store: Arc<dyn MaintenanceStore>,
        filesystem: Arc<dyn FileSystem>,
        write_gate: Arc<WriteGate>,
        startup_recovery: StartupRecoveryServices,
    ) -> Self {
        Self {
            store,
            filesystem,
            configured_library_root: None,
            home_context: None,
            write_gate,
            relocate_plans: Arc::new(Mutex::new(HashMap::new())),
            remove_plans: Arc::new(Mutex::new(HashMap::new())),
            next_plan_id: Arc::new(AtomicU64::new(1)),
            plan_ttl: DEFAULT_PLAN_TTL,
            startup_recovery: StartupRecovery::Configured(startup_recovery),
        }
    }

    /// Test composition without the production source-recovery graph. This
    /// explicit constructor keeps production `new` fail-closed when a
    /// recovery dependency is omitted.
    pub fn for_tests(
        store: Arc<dyn MaintenanceStore>,
        filesystem: Arc<dyn FileSystem>,
        write_gate: Arc<WriteGate>,
    ) -> Self {
        Self {
            store,
            filesystem,
            configured_library_root: None,
            home_context: None,
            write_gate,
            relocate_plans: Arc::new(Mutex::new(HashMap::new())),
            remove_plans: Arc::new(Mutex::new(HashMap::new())),
            next_plan_id: Arc::new(AtomicU64::new(1)),
            plan_ttl: DEFAULT_PLAN_TTL,
            startup_recovery: StartupRecovery::TestOnly,
        }
    }

    pub fn with_library_root(mut self, library_root: PathBuf) -> Self {
        self.configured_library_root = Some(library_root);
        self
    }

    /// Make Home-owned recovery, relocation and removal paths resolve from
    /// the active Bound Home instead of the root chosen at process startup.
    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context.clone());
        self.write_gate = home_context;
        self
    }

    fn active_library_root(&self) -> Result<Option<PathBuf>, MaintenanceError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| Some(home.path))
                .map_err(|error| {
                    MaintenanceError::Internal(format!("active Home unavailable: {error}"))
                }),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn capture_write_context(&self) -> Result<HomeWriteContext, MaintenanceError> {
        self.write_gate
            .capture_open_context()
            .map_err(|_| MaintenanceError::RecoveryInProgress)
    }

    fn acquire_write_guard(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, MaintenanceError> {
        self.write_gate
            .acquire_product_write(context)
            .map_err(|error| match error {
                WriteGateError::Stale => MaintenanceError::PlanStale,
                WriteGateError::Closed => MaintenanceError::RecoveryInProgress,
                other => MaintenanceError::Internal(other.to_string()),
            })
    }

    fn library_root_for_context(
        &self,
        context: &HomeWriteContext,
    ) -> Result<Option<PathBuf>, MaintenanceError> {
        if self.home_context.is_some() {
            Ok(Some(context.home.path.clone()))
        } else {
            self.active_library_root()
        }
    }

    pub fn with_plan_ttl(mut self, plan_ttl: Duration) -> Self {
        self.plan_ttl = plan_ttl;
        self
    }

    pub fn begin_startup(self) -> StartupMaintenance {
        // Keep every composition behind the recovery gate from the moment
        // startup recovery is scheduled. Production enters here already in
        // Recovery; this branch also protects deterministic test and
        // standalone compositions that start from Open.
        if matches!(self.write_gate.snapshot().state, WriteGateState::Open(_))
            && self
                .write_gate
                .transition_to(WriteGateState::Recovery {
                    operation_id: "startup-recovery".into(),
                })
                .is_err()
        {
            // A poisoned transition barrier is itself a fail-closed
            // startup condition; never leave the old Open capability
            // usable when recovery could not claim the gate.
            self.write_gate.mark_blocked();
        }
        let worker_service = self.clone();
        let startup_ready = Arc::new(StartupReadiness::default());
        let worker_ready = startup_ready.clone();
        let worker = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_service.startup_check()
            }))
            .unwrap_or_else(|_| {
                Err(MaintenanceError::Internal(
                    "startup Maintenance task panicked".into(),
                ))
            });
            worker_ready
                .mark_finished(result.is_ok() && worker_service.write_gate.is_product_write_open());
            result
        });
        StartupMaintenance {
            maintenance: self,
            startup_worker: Arc::new(Mutex::new(Some(worker))),
            startup_ready,
        }
    }

    pub fn startup_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        // Direct callers must observe the same ordering as
        // `begin_startup`: close product writes before any journal recovery
        // begins, even when the caller did not use the asynchronous wrapper.
        if matches!(self.write_gate.snapshot().state, WriteGateState::Open(_)) {
            if let Err(error) = self.write_gate.transition_to(WriteGateState::Recovery {
                operation_id: "startup-recovery".into(),
            }) {
                self.write_gate.mark_blocked();
                return Err(MaintenanceError::Internal(error.to_string()));
            }
        }
        // Operation recovery and the reopen only run when the session may
        // write: `Open` (regular) or `Recovery` (the narrow startup-recovery
        // capability). A `CatalogReadOnly` session skips both (spec §4.3:
        // read-only refuses every product write, recovery included).
        if matches!(
            self.write_gate.snapshot().state,
            WriteGateState::Recovery {
                operation_id,
            } if operation_id == "startup-recovery"
        ) {
            self.recover_startup_operations()?;
            self.write_gate.mark_ready();
        }
        // Activation Health Observation is owned by the Observation module
        // (spec §4.10; ADR-0020): the legacy per-row health write here
        // would advance shared `snapshot_version` and race the CAS. The
        // module runs its own read-mostly Target-scoped health on the
        // shared scheduler; skill/entity health remains exposed through
        // explicit maintenance commands.
        Ok(ActivationHealthReport {
            checked: 0,
            snapshot_version: self.write_gate.generation(),
        })
    }

    fn recover_startup_operations(&self) -> Result<(), MaintenanceError> {
        if let StartupRecovery::Configured(recovery) = &self.startup_recovery {
            if let Ok(bound) = self.write_gate.bound_home() {
                recovery
                    .scan_evidence_factory
                    .store_for(&bound)
                    .map_err(|error| MaintenanceError::Internal(error.to_string()))?
                    .cleanup_temporary_runs()
                    .map_err(|error| MaintenanceError::Internal(error.to_string()))?;
            }
        }
        if let Some(library_root) = self.active_library_root()? {
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
                .recover_file_import_journals(&library_root, &baselines)?;
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
                .recover_adopt_journals(&library_root, &baselines, &entities)?;
            let desired_activations = self.store.desired_activation_baselines()?;
            self.filesystem
                .recover_activation_replace_journals(&library_root, &desired_activations)?;
            // Enable Module journals (spec §4.9): per-cell rollback /
            // roll-forward decided by the current catalog desired state.
            let enable_facts = self
                .store
                .activation_cells()?
                .into_iter()
                .map(|cell| crate::seams::filesystem::EnableRecoveryFact {
                    skill_id: cell.skill_id.0,
                    target_root_id: cell.target_root_id,
                    desired_enabled: cell.desired_enabled,
                    expected_entry_path: cell.expected_entry_path,
                    expected_target_path: cell.expected_target_path,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_enable_journals(&library_root, &enable_facts)?;
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
                .recover_relocate_journals(&library_root, &relocate_baselines)?;
            let remove_baselines = self
                .store
                .managed_skill_baselines()?
                .into_iter()
                .map(|baseline| RemoveRecoveryBaseline {
                    skill_id: baseline.skill_id.0,
                })
                .collect::<Vec<_>>();
            self.filesystem
                .recover_remove_journals(&library_root, &remove_baselines)?;
            if let StartupRecovery::Configured(recovery) = &self.startup_recovery {
                recovery.source_transition.recover_pending(&library_root)?;
                recovery.source_lifecycle.recover_lifecycle(&library_root)?;
                // Startup re-verification (spec §8.3): every current member
                // snapshot is compared against the immutable current Source
                // Release; mismatches persist `source_snapshot_mismatch` and
                // block Update, new Enable and ordinary source writes.
                recovery.source_update.verify_all_members()?;
            }
            self.recover_handoff_operations(&library_root)?;
        }
        Ok(())
    }

    /// Ownership Handoff startup recovery (spec §8.4, ADR-0013 §5): every
    /// pending handoff journal decides per item by its phase. Below the
    /// lock CAS the item rolls back in place (external directory restored,
    /// staged copy discarded, nothing committed); at or above it the item
    /// rolls forward under the recovery gate — idempotent Catalog write,
    /// parent manifest backfill, entity publish, Activation flattening —
    /// and the isolation/staged copies are discarded because the restart
    /// closed the Undo window. No long-term double owner or no-owner.
    fn recover_handoff_operations(&self, library_root: &Path) -> Result<(), MaintenanceError> {
        let journals = self.filesystem.list_handoff_journals(library_root)?;
        for mut journal in journals {
            let journal_view = journal.clone();
            for item in &mut journal.items {
                match item.phase {
                    HandoffItemPhase::Planned
                    | HandoffItemPhase::Staged
                    | HandoffItemPhase::SourceIsolated => {
                        // Pre-CAS: roll back in place — EXCEPT for the
                        // crash window between the exact-entry CAS and the
                        // durable journal phase write. The journal still
                        // says SourceIsolated there, but the released entry
                        // proves the commit point passed: resolve the
                        // direction from the lock itself (spec §8.4: the
                        // CAS is the commit point; CAS 后只 roll-forward).
                        let releases_lock = !item.lock_path.as_os_str().is_empty();
                        let cas_happened = if releases_lock {
                            !self
                                .filesystem
                                .lock_entry_present(&item.lock_path, &item.lock_entry_name)?
                        } else {
                            false
                        };
                        if cas_happened {
                            self.roll_forward_handoff_item(library_root, &journal_view, item)?;
                        } else {
                            self.filesystem.rollback_handoff_item(library_root, item)?;
                        }
                    }
                    HandoffItemPhase::OwnershipReleased
                    | HandoffItemPhase::ManagedCommitted
                    | HandoffItemPhase::Finalized => {
                        // Post-CAS: roll forward only. The Catalog write is
                        // idempotent; the manifest is backfilled from the
                        // authoritative row.
                        self.roll_forward_handoff_item(library_root, &journal_view, item)?;
                    }
                }
            }
            self.filesystem
                .write_handoff_journal(library_root, &journal)?;
            self.filesystem
                .finish_handoff_journal(library_root, &journal.operation_id)?;
        }
        Ok(())
    }

    /// Post-CAS roll-forward of one handoff journal item: idempotent
    /// Catalog write (with parent manifest backfill) followed by the
    /// filesystem-side publish, Activation flattening and cleanup.
    fn roll_forward_handoff_item(
        &self,
        library_root: &Path,
        journal_view: &HandoffJournal,
        item: &mut HandoffJournalItem,
    ) -> Result<(), MaintenanceError> {
        let remotes_root = library_root.join("remotes");
        if let Some(remote) = &item.remote {
            let record = self.handoff_recovered_record(item)?;
            self.store.insert_handoff_recovered(record)?;
            let parent = self
                .store
                .find_remote_parent_by_url(&remote.canonical_url)?;
            if let Some(parent) = parent {
                if self
                    .filesystem
                    .read_remote_parent_manifest(&remotes_root, &parent.remote_id)?
                    .is_none()
                {
                    self.filesystem.write_remote_parent_manifest(
                        &remotes_root,
                        &RemoteParentManifest {
                            member_plugins: Default::default(),
                            schema_version: 1,
                            remote_id: parent.remote_id.clone(),
                            canonical_url: parent.canonical_url.clone(),
                            provider: None,
                            tracking_mode: None,
                            tracking_value: None,
                            current_selected_ref: None,
                            current_release_id: None,
                            aliases: parent.aliases.clone(),
                            created_at: parent.created_at.clone(),
                        },
                    )?;
                }
            }
        }
        self.filesystem
            .roll_forward_handoff_item(library_root, journal_view, item)?;
        Ok(())
    }

    /// Rebuild the durable Catalog write a committed Handoff needs from its
    /// journal (spec §8.4 step 5): every fact was frozen at plan time, so
    /// roll-forward never re-fetches or re-verifies the remote.
    fn handoff_recovered_record(
        &self,
        item: &HandoffJournalItem,
    ) -> Result<HandoffRecoveredRecord, MaintenanceError> {
        let remote = item.remote.as_ref().ok_or_else(|| {
            MaintenanceError::Internal("a remote handoff item has no remote journal".into())
        })?;
        Ok(HandoffRecoveredRecord {
            skill: RemoteImportRecord {
                skill_id: SkillId(item.skill_id.clone()),
                directory_name: item.directory_name.clone(),
                identity_key: item.identity_key.clone(),
                display_name: item.display_name.clone(),
                description: item.description.clone(),
                library_entry_path: item.final_entity_path.clone(),
                final_entity_path: item.final_entity_path.clone(),
                recorded_content_hash: item.current_baseline_hash.clone(),
                // The store resolves the existing parent by canonical URL;
                // a fresh id is only a placeholder.
                remote_id: item.remote_id.clone().unwrap_or_default(),
                source_url: remote.canonical_url.clone(),
                requested_ref: remote.requested_ref.clone(),
                verification_anchor_commit: remote.verification_anchor_commit.clone(),
                original_commit_known: remote.original_commit_known,
                skill_path: remote.skill_path.clone(),
                provider_hash: remote.provider_hash.clone(),
                remote_baseline_hash: remote.remote_baseline_hash.clone(),
                current_baseline_hash: item.current_baseline_hash.clone(),
            },
            activations: item
                .activations
                .iter()
                .map(|activation| ActivationRecoveryBaseline {
                    skill_id: item.skill_id.clone(),
                    target_root_id: activation.target_root_id.clone(),
                    expected_entry_path: activation.entry_path.clone(),
                    expected_target_path: activation.target_path.clone(),
                })
                .collect(),
        })
    }

    fn run_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let write_context = self.capture_write_context()?;
        let _write_guard = self.acquire_write_guard(&write_context)?;
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
                    target_root_id: activation.target_root_id.clone(),
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
        let write_context = self.capture_write_context()?;
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
                write_context: write_context.clone(),
                gate_generation: write_context.generation,
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
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(MaintenanceError::PlanStale);
        }
        self.write_gate
            .validate_open_context(&plan.write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => MaintenanceError::PlanStale,
                WriteGateError::Closed => MaintenanceError::RecoveryInProgress,
                other => MaintenanceError::Internal(other.to_string()),
            })?;
        drop(plans);

        let library_root = self
            .library_root_for_context(&plan.write_context)?
            .ok_or_else(|| {
                MaintenanceError::Internal("Maintenance library root is not configured".into())
            })?;
        let _write_guard = self.acquire_write_guard(&plan.write_context)?;
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
                    target_root_id: activation.target_root_id.clone(),
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
        if let Some(library_root) = self.active_library_root()? {
            let canonical_library = self.filesystem.normalize_configured_path(&library_root)?;
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
            self.write_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: compensation_errors.join("; "),
            });
        }
        if let Err(error) = self
            .filesystem
            .finish_relocate_journal(library_root, &journal.operation_id)
        {
            self.write_gate.mark_blocked();
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
        let write_context = self.capture_write_context()?;
        let Some(target) = self.store.remove_target(skill_id)? else {
            return Err(MaintenanceError::SkillNotFound(skill_id.0.clone()));
        };
        // ADR-0018 / §8.3: Git Source Members have no independent Remove;
        // the whole Git Repository Source is the only Remove unit. The
        // source group card owns the lifecycle entry (source_lifecycle.rs).
        if self.store.is_git_source_member(skill_id)? {
            return Err(MaintenanceError::Validation(
                "Git Source Members have no independent Remove; remove the whole Git Repository Source from its source group".into(),
            ));
        }
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
                write_context: write_context.clone(),
                gate_generation: write_context.generation,
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
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(MaintenanceError::PlanStale);
        }
        self.write_gate
            .validate_open_context(&plan.write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => MaintenanceError::PlanStale,
                WriteGateError::Closed => MaintenanceError::RecoveryInProgress,
                other => MaintenanceError::Internal(other.to_string()),
            })?;
        drop(plans);

        let library_root = self
            .library_root_for_context(&plan.write_context)?
            .ok_or_else(|| {
                MaintenanceError::Internal("Maintenance library root is not configured".into())
            })?;
        let _write_guard = self.acquire_write_guard(&plan.write_context)?;
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
                    target_root_id: activation.target_root_id.clone(),
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
        // Last-child semantics (ADR-0013 §4.2): the parent row and its
        // manifest are removed when no other Binding remains; the old
        // external lock owner is never restored.
        let binding_remote_id = self.store.binding_remote_id(&plan.target.skill_id)?;
        let snapshot_version = match self.store.delete_skill(&plan.target.skill_id) {
            Ok(version) => version,
            Err(error) => return self.fail_remove(&library_root, &journal, error.into()),
        };
        if let Some(remote_id) = binding_remote_id {
            let deleted = match self.store.delete_remote_parent_if_last_child(&remote_id) {
                Ok(deleted) => deleted,
                Err(error) => {
                    self.write_gate.mark_blocked();
                    return Err(MaintenanceError::RecoveryRequired {
                        state_error: "the Remove catalog delete committed".into(),
                        compensation_error: format!(
                            "the empty parent could not be deleted: {error}"
                        ),
                    });
                }
            };
            if deleted {
                if let Err(error) = self
                    .filesystem
                    .remove_remote_parent_manifest(&library_root.join("remotes"), &remote_id)
                {
                    self.write_gate.mark_blocked();
                    return Err(MaintenanceError::RecoveryRequired {
                        state_error: "the Remove catalog delete committed".into(),
                        compensation_error: format!(
                            "the parent manifest could not be removed: {error}"
                        ),
                    });
                }
            }
        }
        journal.phase = RemoveJournalPhase::Committed;
        if let Err(error) = self
            .filesystem
            .write_remove_journal(&library_root, &journal)
        {
            self.write_gate.mark_blocked();
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
                self.write_gate.mark_blocked();
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
            self.write_gate.mark_blocked();
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
            self.write_gate.mark_blocked();
            return Err(MaintenanceError::RecoveryRequired {
                state_error: original.to_string(),
                compensation_error: compensation_errors.join("; "),
            });
        }
        if let Err(error) = self
            .filesystem
            .finish_remove_journal(library_root, &journal.operation_id)
        {
            self.write_gate.mark_blocked();
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
        if self.write_gate.is_product_write_open() {
            Ok(())
        } else {
            Err(MaintenanceError::RecoveryInProgress)
        }
    }
}

pub struct StartupMaintenance {
    maintenance: MaintenanceService,
    startup_worker: Arc<Mutex<Option<StartupWorker>>>,
    startup_ready: Arc<StartupReadiness>,
}

type StartupWorker = JoinHandle<Result<ActivationHealthReport, MaintenanceError>>;

impl Clone for StartupMaintenance {
    fn clone(&self) -> Self {
        Self {
            maintenance: self.maintenance.clone(),
            startup_worker: self.startup_worker.clone(),
            startup_ready: self.startup_ready.clone(),
        }
    }
}

#[derive(Default)]
struct StartupReadiness {
    succeeded: Mutex<bool>,
    callbacks: Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>,
}

impl StartupReadiness {
    fn mark_finished(&self, succeeded: bool) {
        if !succeeded {
            return;
        }
        let Ok(mut ready) = self.succeeded.lock() else {
            return;
        };
        if *ready {
            return;
        };
        *ready = true;
        let callbacks = self
            .callbacks
            .lock()
            .map(|mut callbacks| std::mem::take(&mut *callbacks))
            .unwrap_or_default();
        drop(ready);
        for callback in callbacks {
            std::thread::spawn(callback);
        }
    }

    fn register<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let mut callback: Option<Box<dyn FnOnce() + Send + 'static>> = Some(Box::new(callback));
        let Ok(ready) = self.succeeded.lock() else {
            return;
        };
        if *ready {
            drop(ready);
            if let Some(callback) = callback.take() {
                std::thread::spawn(callback);
            }
            return;
        }
        let Ok(mut callbacks) = self.callbacks.lock() else {
            return;
        };
        if let Some(callback) = callback.take() {
            callbacks.push(callback);
        }
    }
}

impl StartupMaintenance {
    /// Schedule startup observations only after the recovery worker has
    /// reached a terminal state. A failed recovery leaves the gate closed,
    /// so observers remain read-only and cannot publish product writes.
    pub fn after_startup_recovery<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let readiness = self.startup_ready.clone();
        readiness.register(callback);
    }

    pub fn run_activation_health_check(&self) -> Result<ActivationHealthReport, MaintenanceError> {
        let mut startup_worker = self
            .startup_worker
            .lock()
            .map_err(|_| MaintenanceError::Internal("startup Maintenance lock poisoned".into()))?;
        let _worker_result = startup_worker.take().map(|worker| {
            worker.join().unwrap_or_else(|_| {
                Err(MaintenanceError::Internal(
                    "startup Maintenance task panicked".into(),
                ))
            })
        });
        drop(startup_worker);
        // §10.4: a retry after a failed startup recovery must re-run the
        // journal recovery itself, not just a read-only scan — otherwise the
        // lock notice clears while writes stay refused.
        let result = if self.maintenance.write_gate.is_product_write_open() {
            self.maintenance.run_health_check()
        } else {
            self.maintenance.startup_check()
        };
        if result.is_ok() && self.maintenance.write_gate.is_product_write_open() {
            self.startup_ready.mark_finished(true);
        }
        result
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::sync_channel;

    use crate::adapters::macos_fs::MacOsFileSystem;
    use crate::adapters::runtime_catalog::RuntimeCatalogStore;
    use crate::adapters::sqlite::SqliteCatalogStore;
    use crate::core::home::BoundHome;
    use crate::core::write_gate::WriteGate;

    use super::*;

    #[test]
    fn startup_observers_wait_until_recovery_reopens_product_writes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            dir.path().join("home"),
        );
        let filesystem = Arc::new(MacOsFileSystem::new(dir.path().to_path_buf()));
        filesystem
            .ensure_directory(&home.path)
            .expect("home directory");
        let sqlite = SqliteCatalogStore::create_bound(&home, &home.path.join("skill-man.sqlite3"))
            .expect("bound Catalog");
        let store = Arc::new(RuntimeCatalogStore::new(
            Arc::new(sqlite),
            filesystem.clone(),
        ));
        let gate = Arc::new(WriteGate::new(WriteGateState::Open(home.clone())));

        let startup = MaintenanceService::for_tests(store, filesystem, gate.clone())
            .with_library_root(home.path.clone())
            .begin_startup();
        let (ready_tx, ready_rx) = sync_channel(1);
        startup.after_startup_recovery(move || {
            ready_tx
                .send(gate.is_product_write_open())
                .expect("observer readiness");
        });

        assert!(
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("startup observer should run"),
            "health/probe/detection must start only after product writes reopen"
        );
    }

    #[test]
    fn failed_startup_recovery_does_not_start_observer_callback() {
        let readiness = Arc::new(StartupReadiness::default());
        let (started_tx, started_rx) = sync_channel(1);
        readiness.register(move || {
            started_tx.send(()).expect("observer callback");
        });
        readiness.mark_finished(false);
        assert!(
            started_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "observers stay idle after failed recovery"
        );
        readiness.mark_finished(true);
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("observers start after recovery retry");
    }
}
