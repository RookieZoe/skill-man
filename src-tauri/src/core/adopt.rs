//! Adopt module (ADR-0005, ADR-0013, spec §8): the read-only evidence
//! ledger scan/plan (see `adopt_evidence`) classifies every candidate as
//! Local / Verified / Modified / Conflict / Deferred / Blocked / Excluded
//! and freezes evidence generation-bound plans; `apply` executes stable
//! Local Link registrations with the durable per-Skill journal machinery
//! below, and `undo`/`finalize` close the result window. Real-directory
//! migration and lock/Home ownership changes were superseded by the vNext
//! Local Link model and the Ownership Handoff ticket respectively.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::Health;
use crate::core::domain::{AgentId, AgentKind, SkillId, parse_skill_metadata};
use crate::core::import::LibraryConflict;
use crate::core::write_gate::{PlanCheck, PlanTicket, WriteGate};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedActivation, AdoptedSkillRecord,
    RemoteAdoptedSkillRecord,
};
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, AdoptActivationStep, AdoptAppearanceKind as JournalAppearanceKind,
    AdoptAppearanceStep, AdoptItemPhase, AdoptJournal, AdoptJournalItem, AdoptJournalKind,
    AdoptJournalPhase, DirectoryFingerprint, FileSystem, FileSystemError, HandoffItemPhase,
    HandoffJournal, HandoffJournalItem, HandoffRemoteJournal, RemoteParentManifest,
    StagedTreeSnapshot,
};
use crate::seams::installer_lock_store::{
    EmptyInstallerLockStore, InstallerLockError, InstallerLockStore, LockEntry, LockReleaseError,
};
use crate::seams::remote_provider::{
    RemoteProvider, RemoteProviderError, UnavailableRemoteProvider,
};

mod adopt_evidence;
pub use adopt_evidence::*;

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);
const DISK_SPACE_RESERVE_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ADOPT_SKILLS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdoptAppearanceKind {
    RealDirectory,
    Symlink { original_target: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptAppearance {
    pub entry_path: PathBuf,
    pub kind: AdoptAppearanceKind,
    /// The Agent whose skills directory holds this entry; `None` for shared.
    pub agent_id: Option<AgentId>,
    /// Present only when this appearance itself is in the configuration's
    /// unique Activation Target. Scan-only appearances never become writes.
    pub target_root_id: Option<String>,
    pub shared: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptPlanKind {
    /// The entity is moved into the Library (file Install).
    Migrate,
    /// The entity stays outside; the Library records a pointer (Link).
    Link,
    /// The entity becomes a Managed remote Install in the Library
    /// (Ownership Handoff, spec §8.4).
    RemoteInstall,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptSkillResult {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub adopted: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptResult {
    pub operation_id: String,
    pub items: Vec<AdoptSkillResult>,
    pub snapshot_version: u64,
    pub undo_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptUndoItemResult {
    pub directory_name: String,
    pub undone: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptUndoResult {
    pub operation_id: String,
    pub items: Vec<AdoptUndoItemResult>,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum AdoptError {
    #[error("{0}")]
    Validation(String),
    #[error("the Adopt preview is stale; rescan before applying")]
    PlanStale,
    #[error("the Adopt preview was not found or expired")]
    PlanNotFound,
    /// The installer lock changed during Apply: the CAS refused, the
    /// failed item was restored, and the remaining uncommitted batch items
    /// are stopped (spec §8.4 step 4, ADR-0013 §5.4).
    #[error("the installer lock changed concurrently: {0}")]
    LockConcurrentChange(String),
    #[error("Adopt requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Store(#[from] AdoptStoreError),
    #[error(transparent)]
    Lock(#[from] InstallerLockError),
    #[error(transparent)]
    LockRelease(#[from] LockReleaseError),
    #[error(transparent)]
    RemoteProvider(#[from] RemoteProviderError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error("internal Adopt error: {0}")]
    Internal(String),
}

#[derive(Clone)]
struct PlannedAdoptItem {
    skill_id: SkillId,
    directory_name: String,
    identity_key: String,
    display_name: String,
    description: String,
    kind: AdoptPlanKind,
    canonical_entity: PathBuf,
    final_entity_path: PathBuf,
    staged_root: PathBuf,
    source_snapshot: StagedTreeSnapshot,
    staged_snapshot: Option<StagedTreeSnapshot>,
    appearances: Vec<AdoptAppearance>,
    activations: Vec<AdoptActivationStep>,
    journal: AdoptJournalItem,
    handoff: Option<PlannedHandoff>,
}

/// The per-item input the durable batch machinery needs; produced by the
/// evidence plan for stable Local Link intents.
#[derive(Clone)]
struct PlanInputItem {
    directory_name: String,
    kind: AdoptPlanKind,
    canonical_entity: PathBuf,
    final_entity_path: PathBuf,
    appearances: Vec<AdoptAppearance>,
    target_agents: Vec<AdoptAgent>,
    handoff: Option<HandoffPlanInput>,
}

/// The frozen Ownership Handoff intent for one plan item (spec §8.4): the
/// strict lock entry, its full-file fingerprint, the remote evidence and
/// the user's explicit choices. Nothing here is writable by scan/plan.
#[derive(Clone)]
struct HandoffPlanInput {
    intent: AdoptPlanIntent,
    lock_path: PathBuf,
    lock_fingerprint: String,
    lock_entry: LockEntry,
    lock_entry_json: String,
    remote: Option<AdoptRemoteEvidence>,
    target_directory: Option<PathBuf>,
}

/// The running handoff state for one apply item: everything frozen at plan
/// time plus the isolation path and staged facts written during Apply.
#[derive(Clone)]
struct PlannedHandoff {
    intent: AdoptPlanIntent,
    lock_path: PathBuf,
    lock_fingerprint: String,
    lock_entry: LockEntry,
    lock_entry_json: String,
    remote: Option<AdoptRemoteEvidence>,
    target_directory: Option<PathBuf>,
    isolated_path: Option<PathBuf>,
    staged_snapshot: Option<StagedTreeSnapshot>,
}

#[derive(Clone)]
struct PlannedAdoptBatch {
    operation_id: String,
    items: Vec<PlannedAdoptItem>,
    journal: AdoptJournal,
    handoff_journal: Option<HandoffJournal>,
    gate_generation: u64,
    created_at_millis: u128,
}

pub struct AdoptService {
    store: Arc<dyn AdoptStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    lock_store: Arc<dyn InstallerLockStore>,
    remote_provider: Arc<dyn RemoteProvider>,
    configured_library_root: PathBuf,
    /// Production reads the current verified Home through this context;
    /// standalone test compositions keep their explicit construction root.
    home_context: Option<Arc<WriteGate>>,
    home_directory: PathBuf,
    plans: Mutex<HashMap<String, PlannedAdoptBatch>>,
    evidence_plans: Mutex<HashMap<String, EvidencePlannedBatch>>,
    applied: Mutex<HashMap<String, PlannedAdoptBatch>>,
    next_plan_id: AtomicU64,
    next_skill_id: AtomicU64,
    evidence_generation: AtomicU64,
    last_report: Mutex<Option<AdoptEvidenceReport>>,
    plan_ttl: Duration,
    write_gate: Arc<WriteGate>,
}

impl AdoptService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn AdoptStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
        home_directory: PathBuf,
    ) -> Self {
        Self {
            store,
            filesystem,
            clock,
            // Fail closed by default: without an explicit lock store the
            // scan sees no lock declarations (Local verdicts); without an
            // explicit provider no remote verification can fabricate
            // evidence. The composition root wires the system adapters.
            lock_store: Arc::new(EmptyInstallerLockStore),
            remote_provider: Arc::new(UnavailableRemoteProvider),
            configured_library_root: library_root,
            home_context: None,
            home_directory,
            plans: Mutex::new(HashMap::new()),
            evidence_plans: Mutex::new(HashMap::new()),
            applied: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            next_skill_id: AtomicU64::new(1),
            evidence_generation: AtomicU64::new(0),
            last_report: Mutex::new(None),
            plan_ttl: DEFAULT_PLAN_TTL,
            write_gate: Arc::new(WriteGate::open_for_tests()),
        }
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.write_gate = write_gate;
        self
    }

    /// Make Home-scoped plans resolve their paths from the bootstrap-verified
    /// Home rather than from the process's startup configuration.
    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context);
        self
    }

    pub fn with_lock_store(mut self, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        self.lock_store = lock_store;
        self
    }

    pub fn with_remote_provider(mut self, remote_provider: Arc<dyn RemoteProvider>) -> Self {
        self.remote_provider = remote_provider;
        self
    }

    fn active_library_root(&self) -> Result<PathBuf, AdoptError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| AdoptError::Internal(format!("active Home unavailable: {error}"))),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn build_planned_batch(
        &self,
        plan_token: &str,
        items: Vec<PlanInputItem>,
        agents: &[AdoptAgent],
    ) -> Result<PlannedAdoptBatch, AdoptError> {
        // The operation id derives from the plan token so every plan maps to
        // exactly one durable operation id (deterministic under a fixed
        // clock, stable across plan/batch registration).
        let plan_number = plan_token.strip_prefix("adopt-plan-").unwrap_or("0");
        let operation_id = format!("adopt-{}-{plan_number}", self.clock.unix_epoch_nanos());
        let staging_operation_root = self
            .active_library_root()?
            .join("staging")
            .join(&operation_id);
        // Ownership Handoff items keep their own durable journal and
        // staging operation (spec §8.4); both derive from the plan number.
        let handoff_operation_id = format!("handoff-{plan_number}");
        let handoff_staging_root = self
            .active_library_root()?
            .join("staging")
            .join(&handoff_operation_id);
        let mut planned_items = Vec::with_capacity(items.len());
        let mut handoff_journal_items: Vec<HandoffJournalItem> = Vec::new();
        for mut item in items {
            let mut activations = Vec::new();
            let final_entity_path = match &item.handoff {
                Some(handoff)
                    if matches!(
                        handoff.intent,
                        AdoptPlanIntent::RemoteInstallKeepCurrent
                            | AdoptPlanIntent::RemoteInstallDiscardModified
                    ) =>
                {
                    self.active_library_root()?
                        .join("skills")
                        .join(&item.directory_name)
                }
                Some(handoff)
                    if matches!(
                        handoff.intent,
                        AdoptPlanIntent::RemoteInstallConvertToLink
                            | AdoptPlanIntent::LocalLinkWithMove
                    ) =>
                {
                    handoff.target_directory.clone().unwrap_or_default()
                }
                _ => item.final_entity_path.clone(),
            };
            for appearance in &item.appearances {
                if appearance.shared || appearance.target_root_id.is_none() {
                    continue;
                }
                activations.push(AdoptActivationStep {
                    target_root_id: appearance.target_root_id.clone().expect("checked above"),
                    // The scan already returned absolute entry paths; they must
                    // stay unresolved (the entry IS the symlink being replaced).
                    entry_path: appearance.entry_path.clone(),
                    target_path: final_entity_path.clone(),
                });
            }
            for agent in agents.iter().filter(|agent| {
                agent.activation_target
                    && item
                        .target_agents
                        .iter()
                        .any(|target| target.agent_id == agent.agent_id)
            }) {
                activations.push(AdoptActivationStep {
                    target_root_id: agent.root_id.clone(),
                    entry_path: self
                        .filesystem
                        .normalize_configured_path(&agent.skills_path.join(&item.directory_name))?,
                    target_path: final_entity_path.clone(),
                });
            }
            let source_snapshot = self
                .filesystem
                .staged_tree_snapshot(&item.canonical_entity)?;
            if matches!(item.kind, AdoptPlanKind::Migrate) {
                crate::core::import::validate_staged_tree(
                    self.filesystem.as_ref(),
                    &source_snapshot,
                )
                .map_err(adopt_validation)?;
            }
            let skill_markdown = self
                .filesystem
                .read_skill_document(&item.canonical_entity)
                .map_err(|error| {
                    AdoptError::Validation(format!("SKILL.md is not readable: {error}"))
                })?;
            let metadata = parse_skill_metadata(&skill_markdown);
            let identity_key = crate::core::import::normalize_identity(&item.directory_name)
                .map_err(adopt_validation)?;
            let required_space = if matches!(
                item.kind,
                AdoptPlanKind::Migrate | AdoptPlanKind::RemoteInstall
            ) {
                source_snapshot
                    .total_file_bytes
                    .saturating_mul(2)
                    .saturating_add(DISK_SPACE_RESERVE_BYTES)
            } else {
                0
            };
            if required_space > 0 {
                let available_space = self
                    .filesystem
                    .available_space(&self.active_library_root()?)?;
                if available_space < required_space {
                    return Err(AdoptError::Validation(format!(
                        "Adopt requires {required_space} bytes of free space, but only {available_space} bytes are available"
                    )));
                }
            }
            let skill_id = SkillId(format!(
                "adopt-{}-{}",
                self.clock.unix_epoch_nanos(),
                self.next_skill_id.fetch_add(1, Ordering::Relaxed)
            ));
            let recorded_content_hash = if matches!(
                item.kind,
                AdoptPlanKind::Migrate | AdoptPlanKind::RemoteInstall
            ) {
                source_snapshot.content_hash.clone()
            } else {
                String::new()
            };
            let (staged_root, handoff) = if let Some(handoff_input) = item.handoff.take() {
                let is_home_install = matches!(
                    handoff_input.intent,
                    AdoptPlanIntent::RemoteInstallKeepCurrent
                        | AdoptPlanIntent::RemoteInstallDiscardModified
                );
                let staged_root = if is_home_install {
                    handoff_staging_root.join(&item.directory_name)
                } else {
                    PathBuf::new()
                };
                (
                    staged_root,
                    Some(PlannedHandoff {
                        intent: handoff_input.intent,
                        lock_path: handoff_input.lock_path,
                        lock_fingerprint: handoff_input.lock_fingerprint,
                        lock_entry: handoff_input.lock_entry,
                        lock_entry_json: handoff_input.lock_entry_json,
                        remote: handoff_input.remote,
                        target_directory: handoff_input.target_directory,
                        isolated_path: None,
                        staged_snapshot: None,
                    }),
                )
            } else {
                (
                    if matches!(item.kind, AdoptPlanKind::Migrate) {
                        staging_operation_root.join(&item.directory_name)
                    } else {
                        PathBuf::new()
                    },
                    None,
                )
            };
            let journal = AdoptJournalItem {
                skill_id: skill_id.0.clone(),
                directory_name: item.directory_name.clone(),
                kind: if matches!(item.kind, AdoptPlanKind::Migrate) {
                    AdoptJournalKind::Migrate
                } else {
                    AdoptJournalKind::Link
                },
                staged_root: staged_root.clone(),
                source_fingerprint: matches!(item.kind, AdoptPlanKind::Migrate)
                    .then_some(source_snapshot.root.clone()),
                // Until Apply stages the Skill, this identifies the source.
                // The journal is not persisted until Apply begins.
                staged_fingerprint: source_snapshot.root.clone(),
                final_entity_path: item.final_entity_path.clone(),
                recorded_content_hash,
                original_path: item.canonical_entity.clone(),
                original_filename: item.directory_name.clone(),
                appearances: item
                    .appearances
                    .iter()
                    .map(|appearance| AdoptAppearanceStep {
                        entry_path: appearance.entry_path.clone(),
                        kind: match &appearance.kind {
                            AdoptAppearanceKind::RealDirectory => {
                                JournalAppearanceKind::RealDirectory
                            }
                            AdoptAppearanceKind::Symlink { original_target } => {
                                JournalAppearanceKind::Symlink {
                                    original_target: original_target.clone(),
                                }
                            }
                        },
                    })
                    .collect(),
                activations: activations.clone(),
                phase: AdoptItemPhase::Planned,
                installed_fingerprint: None,
            };
            let planned_item = PlannedAdoptItem {
                skill_id: skill_id.clone(),
                directory_name: item.directory_name.clone(),
                identity_key: identity_key.clone(),
                display_name: metadata
                    .name
                    .clone()
                    .unwrap_or_else(|| item.directory_name.clone()),
                description: metadata.description.unwrap_or_default(),
                kind: item.kind,
                canonical_entity: item.canonical_entity.clone(),
                final_entity_path: final_entity_path.clone(),
                staged_root,
                source_snapshot,
                staged_snapshot: None,
                appearances: item.appearances.clone(),
                activations,
                journal,
                handoff,
            };
            if let Some(handoff) = &planned_item.handoff {
                let remote_journal = handoff.remote.as_ref().map(|remote| HandoffRemoteJournal {
                    canonical_url: remote.canonical_url.clone(),
                    requested_ref: remote.requested_ref.clone(),
                    verification_anchor_commit: remote.anchor_commit.clone(),
                    original_commit_known: remote.original_install_commit_known,
                    skill_path: remote.skill_path.clone(),
                    provider_hash: (!remote.provider_hash.is_empty())
                        .then_some(remote.provider_hash.clone()),
                    remote_baseline_hash: remote.remote_tree_hash.clone(),
                });
                let current_baseline_hash = match handoff.intent {
                    AdoptPlanIntent::RemoteInstallDiscardModified => handoff
                        .remote
                        .as_ref()
                        .map(|remote| remote.remote_tree_hash.clone())
                        .unwrap_or_default(),
                    _ => planned_item.source_snapshot.content_hash.clone(),
                };
                handoff_journal_items.push(HandoffJournalItem {
                    skill_id: planned_item.skill_id.0.clone(),
                    directory_name: planned_item.directory_name.clone(),
                    identity_key: identity_key.clone(),
                    display_name: planned_item.display_name.clone(),
                    description: planned_item.description.clone(),
                    canonical_entity: planned_item.canonical_entity.clone(),
                    isolated_path: None,
                    staged_root: planned_item.staged_root.clone(),
                    staged_fingerprint: None,
                    final_entity_path: planned_item.final_entity_path.clone(),
                    source_tree_hash: planned_item.source_snapshot.content_hash.clone(),
                    staged_tree_hash: None,
                    lock_path: handoff.lock_path.clone(),
                    lock_fingerprint: handoff.lock_fingerprint.clone(),
                    lock_entry_name: handoff.lock_entry.name.clone(),
                    lock_entry_json: handoff.lock_entry_json.clone(),
                    remote_id: None,
                    remote: remote_journal,
                    current_baseline_hash,
                    appearances: planned_item.journal.appearances.clone(),
                    activations: planned_item.activations.clone(),
                    target_directory: handoff.target_directory.clone(),
                    phase: HandoffItemPhase::Planned,
                });
            }
            planned_items.push(planned_item);
        }
        let journal = AdoptJournal {
            version: 2,
            operation_id: operation_id.clone(),
            phase: AdoptJournalPhase::Planned,
            staging_operation_root: staging_operation_root.clone(),
            // Apply writes the intent journal before creating staging, then
            // replaces this sentinel with the owned directory fingerprint.
            staging_fingerprint: DirectoryFingerprint {
                canonical_path: staging_operation_root,
                device: 0,
                inode: 0,
            },
            // Ownership Handoff items are tracked by their own journal and
            // must never appear in the Adopt journal (recovery would apply
            // the wrong semantics); the apply loop keeps separate cursors.
            items: planned_items
                .iter()
                .filter(|item| item.handoff.is_none())
                .map(|item| item.journal.clone())
                .collect(),
        };
        let handoff_journal = (!handoff_journal_items.is_empty()).then(|| HandoffJournal {
            version: 1,
            operation_id: handoff_operation_id.clone(),
            staging_operation_root: handoff_staging_root.clone(),
            // Apply writes the intent journal before creating staging, then
            // replaces this sentinel with the owned directory fingerprint.
            staging_fingerprint: DirectoryFingerprint {
                canonical_path: handoff_staging_root,
                device: 0,
                inode: 0,
            },
            items: handoff_journal_items,
        });
        let batch = PlannedAdoptBatch {
            operation_id,
            items: planned_items,
            journal,
            handoff_journal,
            gate_generation: self.write_gate.generation(),
            created_at_millis: self.clock.monotonic_millis(),
        };
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?;
        plans.insert(plan_token.to_owned(), batch.clone());
        Ok(batch)
    }

    /// Apply a plan. Evidence plans are freeze-checked first (any rescan or
    /// external change makes them stale before any write), then the durable
    /// per-Skill machinery runs: each Skill is its own transaction with its
    /// own rollback; failures leave the other Skills (and their Activations)
    /// intact.
    pub fn apply(&self, plan_token: &str) -> Result<AdoptResult, AdoptError> {
        self.ensure_writes_ready()?;
        // Evidence dispatch: re-verify the frozen world for every applyable
        // item before the first write (spec §8.1 TOCTOU row).
        {
            let mut evidence_plans = self
                .evidence_plans
                .lock()
                .map_err(|_| AdoptError::Internal("Adopt evidence plan lock poisoned".into()))?;
            if let Some(batch) = evidence_plans.remove(plan_token) {
                for item in &batch.items {
                    // Every intent re-verifies the frozen world before the
                    // first write (spec §8.1 TOCTOU row): the generation,
                    // entity identity, tree, lock bytes and appearances.
                    self.recheck_frozen_evidence(&item.frozen)?;
                }
            }
        }
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?;
        let mut batch = plans.remove(plan_token).ok_or(AdoptError::PlanNotFound)?;
        if self.write_gate.check_plan(PlanTicket {
            generation: batch.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(AdoptError::PlanStale);
        }
        drop(plans);
        if self
            .clock
            .monotonic_millis()
            .saturating_sub(batch.created_at_millis)
            >= self.plan_ttl.as_millis()
        {
            return Err(AdoptError::PlanNotFound);
        }
        self.preflight_batch(&batch)?;
        let mut journal = batch.journal.clone();
        let has_adopt_items = !journal.items.is_empty();
        if has_adopt_items {
            journal.phase = AdoptJournalPhase::Applying;
            if let Err(error) = self
                .filesystem
                .write_adopt_journal(&self.active_library_root()?, &journal)
            {
                return Err(self.block_for_recovery("persist Adopt intent", error));
            }
            journal.staging_fingerprint = match self
                .filesystem
                .create_adopt_staging_operation(&self.active_library_root()?, &journal.operation_id)
            {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    return Err(self.block_for_recovery("prepare Adopt staging", error));
                }
            };
            if let Err(error) = self
                .filesystem
                .write_adopt_journal(&self.active_library_root()?, &journal)
            {
                return Err(self.block_for_recovery("persist Adopt staging intent", error));
            }
        }
        let mut handoff_journal = batch.handoff_journal.clone();
        if let Some(handoff_journal) = handoff_journal.as_mut() {
            if let Err(error) = self
                .filesystem
                .write_handoff_journal(&self.active_library_root()?, handoff_journal)
            {
                return Err(self.block_for_recovery("persist Handoff intent", error));
            }
            handoff_journal.staging_fingerprint =
                match self.filesystem.create_adopt_staging_operation(
                    &self.active_library_root()?,
                    &handoff_journal.operation_id,
                ) {
                    Ok(fingerprint) => fingerprint,
                    Err(error) => {
                        return Err(self.block_for_recovery("prepare Handoff staging", error));
                    }
                };
            if let Err(error) = self
                .filesystem
                .write_handoff_journal(&self.active_library_root()?, handoff_journal)
            {
                return Err(self.block_for_recovery("persist Handoff staging intent", error));
            }
        }
        let mut results = Vec::with_capacity(batch.items.len());
        let mut snapshot_version = 0_u64;
        let mut adopt_index = 0_usize;
        let mut handoff_index = 0_usize;
        let mut lock_changed = false;
        // Per lock path, the fingerprint the next in-batch CAS may expect:
        // the frozen plan fingerprint first, then the fingerprint our own
        // release rewrote (spec §8.4: multiple Skills of one lock are each
        // released; an external rewrite between items still refuses).
        let mut expected_lock_fingerprints: HashMap<PathBuf, String> = HashMap::new();
        for index in 0..batch.items.len() {
            if lock_changed {
                results.push(AdoptSkillResult {
                    skill_id: batch.items[index].skill_id.clone(),
                    directory_name: batch.items[index].directory_name.clone(),
                    adopted: false,
                    error: Some(
                        "the installer lock changed concurrently; the remaining items were not applied"
                            .into(),
                    ),
                });
                continue;
            }
            let is_handoff = batch.items[index].handoff.is_some();
            let outcome = {
                let item = &mut batch.items[index];
                if is_handoff {
                    self.apply_handoff_item(
                        handoff_journal.as_mut(),
                        &mut expected_lock_fingerprints,
                        handoff_index,
                        item,
                    )
                    .map(|version| (version, ()))
                } else {
                    self.prepare_item_for_apply(&mut journal, adopt_index, item)
                        .and_then(|()| self.apply_item(&mut journal, adopt_index, item))
                        .map(|version| (version, ()))
                }
            };
            let item = &batch.items[index];
            match outcome {
                Ok((version, _)) => {
                    snapshot_version = version;
                    results.push(AdoptSkillResult {
                        skill_id: item.skill_id.clone(),
                        directory_name: item.directory_name.clone(),
                        adopted: true,
                        error: None,
                    });
                }
                Err(error) => {
                    let rollback = if is_handoff {
                        if matches!(error, AdoptError::RecoveryRequired(_)) {
                            // Post-commit failures can only roll forward
                            // under the recovery gate; the committed state
                            // stays exactly as the journal records it.
                            Ok(())
                        } else {
                            self.rollback_handoff_item(handoff_journal.as_mut(), handoff_index)
                        }
                    } else {
                        self.rollback_item(&mut journal, adopt_index, item)
                    };
                    match rollback {
                        Ok(()) => {
                            if matches!(error, AdoptError::LockConcurrentChange(_)) {
                                lock_changed = true;
                            }
                            results.push(AdoptSkillResult {
                                skill_id: item.skill_id.clone(),
                                directory_name: item.directory_name.clone(),
                                adopted: false,
                                error: Some(error.to_string()),
                            });
                        }
                        Err(compensation) => {
                            return Err(self.block_for_recovery(
                                "roll back failed Adopt item",
                                format!("{error}; rollback also failed: {compensation}"),
                            ));
                        }
                    }
                }
            }
            if is_handoff {
                handoff_index += 1;
            } else {
                batch.items[index].journal = journal.items[adopt_index].clone();
                adopt_index += 1;
            }
        }
        if has_adopt_items {
            if let Err(error) = self.filesystem.discard_staging(
                &journal.staging_operation_root,
                &self.active_library_root()?,
                Some(&journal.staging_fingerprint),
            ) {
                return Err(self.block_for_recovery("clean Adopt staging after Apply", error));
            }
            journal.phase = AdoptJournalPhase::Committed;
            if let Err(error) = self
                .filesystem
                .write_adopt_journal(&self.active_library_root()?, &journal)
            {
                return Err(self.block_for_recovery("persist committed Adopt batch", error));
            }
        }
        if let Some(handoff_journal) = handoff_journal.as_mut() {
            // The journal stays live for the conditional Undo window; the
            // staged copies are gone (installed or rolled back) and the
            // isolation copy is the short-term Undo copy (spec §8.4 step 6).
            batch.handoff_journal = Some(handoff_journal.clone());
            if let Err(error) = self.filesystem.discard_staging(
                &handoff_journal.staging_operation_root,
                &self.active_library_root()?,
                Some(&handoff_journal.staging_fingerprint),
            ) {
                return Err(self.block_for_recovery("clean Handoff staging after Apply", error));
            }
        }
        batch.journal = journal.clone();
        let undo_available = results.iter().any(|result| result.adopted);
        let operation_id = batch.operation_id.clone();
        if undo_available {
            match self.applied.lock() {
                Ok(mut applied) => {
                    applied.insert(batch.operation_id.clone(), batch);
                }
                Err(_) => {
                    return Err(self.block_for_recovery(
                        "retain committed Adopt batch for Undo",
                        "Adopt applied lock poisoned",
                    ));
                }
            }
        } else {
            if has_adopt_items {
                if let Err(error) = self
                    .filesystem
                    .finish_adopt_journal(&self.active_library_root()?, &operation_id)
                {
                    return Err(self.block_for_recovery("archive empty Adopt batch", error));
                }
            }
            if let Some(handoff_journal) = &batch.handoff_journal {
                if let Err(error) = self.filesystem.finish_handoff_journal(
                    &self.active_library_root()?,
                    &handoff_journal.operation_id,
                ) {
                    return Err(self.block_for_recovery("archive empty Handoff batch", error));
                }
            }
        }
        Ok(AdoptResult {
            operation_id,
            items: results,
            snapshot_version,
            undo_available,
        })
    }

    fn preflight_batch(&self, batch: &PlannedAdoptBatch) -> Result<(), AdoptError> {
        for item in &batch.items {
            let current = self
                .filesystem
                .staged_tree_snapshot(&item.canonical_entity)
                .map_err(|_| AdoptError::PlanStale)?;
            if current != item.source_snapshot {
                return Err(AdoptError::PlanStale);
            }
            if matches!(item.kind, AdoptPlanKind::Migrate) {
                crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &current)
                    .map_err(adopt_validation)?;
            }
        }
        Ok(())
    }

    fn prepare_item_for_apply(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &mut PlannedAdoptItem,
    ) -> Result<(), AdoptError> {
        if matches!(item.kind, AdoptPlanKind::Link) {
            let current = self
                .filesystem
                .staged_tree_snapshot(&item.canonical_entity)
                .map_err(|_| AdoptError::PlanStale)?;
            if current != item.source_snapshot {
                return Err(AdoptError::PlanStale);
            }
            journal.items[index].phase = AdoptItemPhase::Staged;
            self.filesystem
                .write_adopt_journal(&self.active_library_root()?, journal)?;
            return Ok(());
        }

        let staged_fingerprint = self
            .filesystem
            .stage_external_directory_in_adopt_operation(
                &item.canonical_entity,
                &self.active_library_root()?,
                &journal.operation_id,
                &item.directory_name,
                &journal.staging_fingerprint,
                &item.source_snapshot.root,
            )?;
        journal.items[index].staged_fingerprint = staged_fingerprint;
        journal.items[index].phase = AdoptItemPhase::Staged;
        self.filesystem
            .write_adopt_journal(&self.active_library_root()?, journal)?;

        let staged_snapshot = self.filesystem.staged_tree_snapshot(&item.staged_root)?;
        if staged_snapshot.content_hash != item.source_snapshot.content_hash
            || staged_snapshot.total_file_bytes != item.source_snapshot.total_file_bytes
        {
            return Err(AdoptError::PlanStale);
        }
        crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &staged_snapshot)
            .map_err(adopt_validation)?;
        item.staged_snapshot = Some(staged_snapshot);
        Ok(())
    }

    fn apply_item(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<u64, AdoptError> {
        if let Some(snapshot) = &item.staged_snapshot {
            let fingerprint = self.filesystem.install_staged_skill(
                &item.staged_root,
                &item.final_entity_path,
                &self.active_library_root()?,
                &journal.operation_id,
                snapshot,
            )?;
            let entry = &mut journal.items[index];
            entry.installed_fingerprint = Some(fingerprint);
            entry.phase = AdoptItemPhase::EntityInstalled;
            self.filesystem
                .write_adopt_journal(&self.active_library_root()?, journal)?;
        }
        let record = AdoptedSkillRecord {
            skill_id: item.skill_id.clone(),
            directory_name: item.directory_name.clone(),
            identity_key: item.identity_key.clone(),
            display_name: item.display_name.clone(),
            description: item.description.clone(),
            library_entry_path: item
                .staged_snapshot
                .as_ref()
                .map(|_| item.final_entity_path.clone()),
            final_entity_path: item.final_entity_path.clone(),
            recorded_content_hash: item
                .staged_snapshot
                .as_ref()
                .map(|snapshot| snapshot.content_hash.clone()),
            original_path: item
                .staged_snapshot
                .as_ref()
                .map(|_| item.canonical_entity.clone()),
            original_filename: item.directory_name.clone(),
            activations: item
                .activations
                .iter()
                .map(|activation| AdoptedActivation {
                    target_root_id: activation.target_root_id.clone(),
                    expected_entry_path: activation.entry_path.clone(),
                    expected_target_path: activation.target_path.clone(),
                })
                .collect(),
        };
        let snapshot_version = self.store.insert_adopted(record)?;
        journal.items[index].phase = AdoptItemPhase::CatalogCommitted;
        self.filesystem
            .write_adopt_journal(&self.active_library_root()?, journal)?;
        self.filesystem
            .apply_adopt_appearances(&journal.items[index].appearances, &item.activations)?;
        journal.items[index].phase = AdoptItemPhase::Done;
        self.filesystem
            .write_adopt_journal(&self.active_library_root()?, journal)?;
        if let Some(source_fingerprint) = &journal.items[index].source_fingerprint {
            self.filesystem.discard_isolated_adopt_source(
                &item.canonical_entity,
                &journal.operation_id,
                source_fingerprint,
            )?;
        }
        Ok(snapshot_version)
    }

    fn rollback_item(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<(), AdoptError> {
        let phase = journal.items[index].phase;
        for activation in item.activations.iter().rev() {
            if matches!(
                self.filesystem.activation_snapshot(&activation.entry_path)?,
                ActivationEntrySnapshot::Symlink { target }
                    if target == activation.target_path
            ) {
                self.filesystem.remove_activation(&activation.entry_path)?;
            }
        }
        if matches!(
            phase,
            AdoptItemPhase::CatalogCommitted
                | AdoptItemPhase::AppearancesApplied
                | AdoptItemPhase::Done
        ) {
            self.store.remove_adopted_skill(&item.skill_id)?;
        }
        self.restore_entity_and_appearances(item, &journal.items[index], &journal.operation_id)
            .map_err(|error| {
                AdoptError::RecoveryRequired(format!(
                    "the entity could not be restored to its original location: {error}"
                ))
            })?;
        let entry = &mut journal.items[index];
        entry.installed_fingerprint = None;
        entry.phase = AdoptItemPhase::Planned;
        self.filesystem
            .write_adopt_journal(&self.active_library_root()?, journal)?;
        Ok(())
    }

    /// Ownership Handoff per-Skill state machine (spec §8.4, ADR-0013 §5):
    /// Planned → Staged → Source Isolated → External Ownership Released
    /// (the exact lock-entry CAS; the logical commit point) → Managed
    /// Committed → Finalized. Every pre-commit step rolls back in place on
    /// error; every post-commit failure blocks for recovery (roll-forward
    /// only). A CAS refusal restores the isolated source and stops the
    /// remaining uncommitted batch items.
    fn apply_handoff_item(
        &self,
        handoff_journal: Option<&mut HandoffJournal>,
        expected_lock_fingerprints: &mut HashMap<PathBuf, String>,
        index: usize,
        item: &mut PlannedAdoptItem,
    ) -> Result<u64, AdoptError> {
        let handoff_journal = handoff_journal
            .ok_or_else(|| AdoptError::Internal("a handoff item has no handoff journal".into()))?;
        let handoff = item
            .handoff
            .clone()
            .ok_or_else(|| AdoptError::Internal("a handoff item has no handoff plan".into()))?;
        let is_home_install = matches!(
            handoff.intent,
            AdoptPlanIntent::RemoteInstallKeepCurrent
                | AdoptPlanIntent::RemoteInstallDiscardModified
        );

        // 1. Staged: copy the current tree (KeepCurrent) or the explicitly
        // chosen Verification Anchor tree (DiscardToAnchor) into the owned
        // staging root; compute hashes and run the Install safety checks.
        if is_home_install {
            let staged_root = handoff_journal.items[index].staged_root.clone();
            let mut workspace: Option<PathBuf> = None;
            let source = match handoff.intent {
                AdoptPlanIntent::RemoteInstallKeepCurrent => item.canonical_entity.clone(),
                AdoptPlanIntent::RemoteInstallDiscardModified => {
                    // Apply re-verifies the remote (spec §8.2.9): the
                    // anchor is pinned, so the materialized tree must equal
                    // the frozen remote tree hash.
                    let request = self
                        .remote_provider
                        .parse_request(&handoff.lock_entry)
                        .map_err(AdoptError::RemoteProvider)?;
                    // The anchor was resolved when the plan froze; upstream
                    // may have moved since, so re-verify the exact frozen
                    // anchor commit, never the new tip (spec §8.2: pinned
                    // commits must match exactly; DiscardToAnchor reinstalls
                    // the anchor, never HEAD).
                    let mut request = request;
                    request.requested_ref = handoff
                        .remote
                        .as_ref()
                        .map(|remote| remote.anchor_commit.clone())
                        .unwrap_or_default();
                    let workspace_dir = self.filesystem.create_temp_workspace("handoff-anchor")?;
                    workspace = Some(workspace_dir.clone());
                    let facts = self
                        .remote_provider
                        .verify(&request, &workspace_dir)
                        .map_err(AdoptError::RemoteProvider)?;
                    if facts.anchor.anchor_commit
                        != handoff
                            .remote
                            .as_ref()
                            .map(|remote| remote.anchor_commit.clone())
                            .unwrap_or_default()
                        || !facts.provider_hash_matched
                    {
                        return Err(AdoptError::PlanStale);
                    }
                    facts.materialized_root
                }
                _ => unreachable!("home install intents only"),
            };
            let staged_result = self.filesystem.copy_tree_verified(&source, &staged_root);
            if let Some(workspace) = &workspace {
                let _ = self.filesystem.discard_temp_workspace(workspace);
            }
            staged_result?;
            let snapshot = self.filesystem.staged_tree_snapshot(&staged_root)?;
            let expected_hash = match handoff.intent {
                AdoptPlanIntent::RemoteInstallKeepCurrent => {
                    handoff_journal.items[index].source_tree_hash.clone()
                }
                AdoptPlanIntent::RemoteInstallDiscardModified => handoff
                    .remote
                    .as_ref()
                    .map(|remote| remote.remote_tree_hash.clone())
                    .unwrap_or_default(),
                _ => unreachable!("home install intents only"),
            };
            if snapshot.content_hash != expected_hash {
                return Err(AdoptError::PlanStale);
            }
            crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &snapshot)
                .map_err(adopt_validation)?;
            handoff_journal.items[index].staged_fingerprint = Some(snapshot.root.clone());
            handoff_journal.items[index].staged_tree_hash = Some(snapshot.content_hash.clone());
            item.handoff
                .as_mut()
                .expect("handoff plan present")
                .staged_snapshot = Some(snapshot);
        }
        handoff_journal.items[index].phase = HandoffItemPhase::Staged;
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;

        // 2. Source Isolated: atomically rename the external canonical
        // directory to its same-parent hidden operation path and re-verify
        // the frozen tree.
        let isolated = self
            .filesystem
            .isolate_external_source(&item.canonical_entity, &handoff_journal.operation_id)?;
        let isolated_snapshot = self.filesystem.staged_tree_snapshot(&isolated)?;
        if isolated_snapshot.content_hash != handoff_journal.items[index].source_tree_hash {
            // The external tree changed during the handoff: restore it in
            // place (the copy under verification is the user's data) and
            // abort this item. The expected hash is the copy's own hash so
            // the restore is unconditional.
            self.filesystem.restore_isolated_source(
                &isolated,
                &item.canonical_entity,
                &isolated_snapshot.content_hash,
            )?;
            return Err(AdoptError::PlanStale);
        }
        handoff_journal.items[index].isolated_path = Some(isolated.clone());
        handoff_journal.items[index].phase = HandoffItemPhase::SourceIsolated;
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
        item.handoff
            .as_mut()
            .expect("handoff plan present")
            .isolated_path = Some(isolated.clone());

        // 3./4. External Ownership Released — the logical commit point:
        // the exact lock-entry CAS (remote intents) or the journaled move
        // itself (Local Link with Move, which has no lock).
        match handoff.intent {
            AdoptPlanIntent::LocalLinkWithMove => {
                handoff_journal.items[index].phase = HandoffItemPhase::OwnershipReleased;
                self.filesystem
                    .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
            }
            _ => {
                // The exact-entry CAS (spec §8.4 step 4). The full-file
                // fingerprint guards against concurrent external rewrites:
                // the first item of a batch compares against the frozen
                // plan fingerprint; every later item compares against the
                // fingerprint our own previous release produced, so a
                // legitimate in-batch rewrite never looks like an external
                // change.
                let expected_fingerprint = expected_lock_fingerprints
                    .get(&handoff.lock_path)
                    .cloned()
                    .unwrap_or_else(|| handoff.lock_fingerprint.clone());
                let released = self.lock_store.release_entry(
                    &handoff.lock_path,
                    &expected_fingerprint,
                    &handoff.lock_entry,
                );
                match released {
                    Ok(()) => {
                        let next_fingerprint = self
                            .lock_store
                            .discover()
                            .map(|reports| {
                                reports
                                    .into_iter()
                                    .find(|report| report.path == handoff.lock_path)
                            })
                            .ok()
                            .flatten()
                            .map(|report| report.fingerprint);
                        if let Some(fingerprint) = next_fingerprint {
                            expected_lock_fingerprints
                                .insert(handoff.lock_path.clone(), fingerprint);
                        }
                        handoff_journal.items[index].phase = HandoffItemPhase::OwnershipReleased;
                        self.filesystem
                            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
                    }
                    Err(error) => {
                        // In-place restore; the lock was never written. The
                        // caller stops the remaining uncommitted items.
                        if let Err(compensation) = self.filesystem.restore_isolated_source(
                            &isolated,
                            &item.canonical_entity,
                            &handoff_journal.items[index].source_tree_hash,
                        ) {
                            return Err(self.block_for_recovery(
                                "restore isolated source after CAS refusal",
                                format!("{error}; compensation: {compensation}"),
                            ));
                        }
                        handoff_journal.items[index].isolated_path = None;
                        handoff_journal.items[index].phase = HandoffItemPhase::Planned;
                        item.handoff
                            .as_mut()
                            .expect("handoff plan present")
                            .isolated_path = None;
                        return Err(AdoptError::LockConcurrentChange(error.to_string()));
                    }
                }
            }
        }

        // 5. Managed Committed: publish the Home entity (or the chosen
        // stable Link directory), commit the Catalog rows, and flatten the
        // real Agent appearances into Activations.
        if is_home_install {
            let staged_snapshot = item
                .handoff
                .as_ref()
                .and_then(|handoff| handoff.staged_snapshot.clone())
                .ok_or_else(|| AdoptError::Internal("the staged snapshot is missing".into()))?;
            self.filesystem.install_staged_skill(
                &handoff_journal.items[index].staged_root,
                &item.final_entity_path,
                &self.active_library_root()?,
                &handoff_journal.operation_id,
                &staged_snapshot,
            )?;
        } else {
            let target = handoff_journal.items[index]
                .target_directory
                .clone()
                .ok_or_else(|| {
                    AdoptError::Internal("a Link handoff has no target directory".into())
                })?;
            if let Some(parent) = target.parent() {
                self.filesystem.ensure_directory(parent)?;
            }
            self.filesystem.restore_isolated_source(
                &isolated,
                &target,
                &handoff_journal.items[index].source_tree_hash,
            )?;
        }
        let snapshot_version = if is_home_install {
            let remote_journal = handoff_journal.items[index].remote.clone().ok_or_else(|| {
                AdoptError::Internal("a remote handoff has no remote journal".into())
            })?;
            let canonical_url = remote_journal.canonical_url.clone();
            let remote_id = self.remote_id_for(&canonical_url)?;
            // A reused parent must be consistent with its manifest before a
            // new Binding commits under it (ADR-0013 §4.3: a mismatched
            // parent closes new Bindings too; only Update/alias are gated
            // elsewhere — this is the handoff's own gate).
            if let Ok(Some(manifest)) = self.filesystem.read_remote_parent_manifest(
                &self.active_library_root()?.join("remotes"),
                &remote_id,
            ) {
                if manifest.remote_id != remote_id || manifest.canonical_url != canonical_url {
                    return Err(AdoptError::Validation(format!(
                        "Remote Source Identity Conflict: the parent '{remote_id}' manifest does \
                         not match the Catalog row; new Bindings are closed for this parent"
                    )));
                }
            }
            handoff_journal.items[index].remote_id = Some(remote_id.clone());
            // The parent manifest is written before the Catalog commit so a
            // crash between the two still leaves a consistent pair; startup
            // roll-forward re-writes it when missing (idempotent).
            let manifest = RemoteParentManifest {
                schema_version: 1,
                remote_id: remote_id.clone(),
                canonical_url: canonical_url.clone(),
                provider: None,
                tracking_mode: None,
                tracking_value: None,
                current_selected_ref: None,
                current_release_id: None,
                // A reused parent keeps its confirmed aliases in the
                // manifest (ADR-0013 §4.3: manifest and row must agree).
                aliases: self
                    .store
                    .find_remote_parent_by_url(&canonical_url)?
                    .map(|parent| parent.aliases)
                    .unwrap_or_default(),
                created_at: crate::seams::clock::iso_timestamp(self.clock.unix_epoch_nanos()),
            };
            self.filesystem.write_remote_parent_manifest(
                &self.active_library_root()?.join("remotes"),
                &manifest,
            )?;
            let current_baseline = handoff_journal.items[index].current_baseline_hash.clone();
            let remote_baseline = remote_journal.remote_baseline_hash.clone();
            let record = RemoteAdoptedSkillRecord {
                skill_id: item.skill_id.clone(),
                directory_name: item.directory_name.clone(),
                identity_key: item.identity_key.clone(),
                display_name: item.display_name.clone(),
                description: item.description.clone(),
                final_entity_path: item.final_entity_path.clone(),
                recorded_content_hash: current_baseline.clone(),
                remote_id,
                canonical_url,
                requested_ref: remote_journal.requested_ref,
                verification_anchor_commit: remote_journal.verification_anchor_commit,
                original_commit_known: remote_journal.original_commit_known,
                skill_path: remote_journal.skill_path,
                provider_hash: remote_journal.provider_hash,
                remote_baseline_hash: remote_baseline.clone(),
                current_baseline_hash: current_baseline.clone(),
                health: if current_baseline == remote_baseline {
                    Health::Healthy
                } else {
                    Health::Modified
                },
                activations: item
                    .activations
                    .iter()
                    .map(|activation| AdoptedActivation {
                        target_root_id: activation.target_root_id.clone(),
                        expected_entry_path: activation.entry_path.clone(),
                        expected_target_path: activation.target_path.clone(),
                    })
                    .collect(),
            };
            self.store.insert_remote_adopted(record)?
        } else {
            let record = AdoptedSkillRecord {
                skill_id: item.skill_id.clone(),
                directory_name: item.directory_name.clone(),
                identity_key: item.identity_key.clone(),
                display_name: item.display_name.clone(),
                description: item.description.clone(),
                library_entry_path: None,
                final_entity_path: item.final_entity_path.clone(),
                recorded_content_hash: Some(handoff_journal.items[index].source_tree_hash.clone()),
                original_path: None,
                original_filename: item.directory_name.clone(),
                activations: item
                    .activations
                    .iter()
                    .map(|activation| AdoptedActivation {
                        target_root_id: activation.target_root_id.clone(),
                        expected_entry_path: activation.entry_path.clone(),
                        expected_target_path: activation.target_path.clone(),
                    })
                    .collect(),
            };
            self.store.insert_adopted(record)?
        };
        self.filesystem.apply_adopt_appearances(
            &handoff_journal.items[index].appearances,
            &item.activations,
        )?;
        handoff_journal.items[index].phase = HandoffItemPhase::ManagedCommitted;
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;

        // 6. Finalized: verify the Catalog, Home entity, private
        // Activations and the absent external canonical path.
        let final_snapshot = self
            .filesystem
            .staged_tree_snapshot(&item.final_entity_path)
            .map_err(|error| {
                self.block_for_recovery("verify handoff final entity", format!("{error}"))
            })?;
        if final_snapshot.content_hash != handoff_journal.items[index].current_baseline_hash {
            return Err(self.block_for_recovery(
                "verify handoff final entity",
                "the Home entity does not match the handoff baseline",
            ));
        }
        if self
            .filesystem
            .path_is_directory(&item.canonical_entity)
            .unwrap_or(false)
        {
            return Err(self.block_for_recovery(
                "verify handoff external release",
                "the external canonical directory reappeared",
            ));
        }
        handoff_journal.items[index].phase = HandoffItemPhase::Finalized;
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
        Ok(snapshot_version)
    }

    /// Pre-commit rollback of one handoff item: restore the external
    /// canonical directory from its isolation copy (the tree must still
    /// match the frozen hash) and discard the staged copy. The lock and
    /// Catalog were never touched below the commit point.
    fn rollback_handoff_item(
        &self,
        handoff_journal: Option<&mut HandoffJournal>,
        index: usize,
    ) -> Result<(), AdoptError> {
        let handoff_journal = handoff_journal
            .ok_or_else(|| AdoptError::Internal("a handoff item has no handoff journal".into()))?;
        if let Err(error) = self.filesystem.rollback_handoff_item(
            &self.active_library_root()?,
            &mut handoff_journal.items[index],
        ) {
            return Err(self.block_for_recovery("roll back handoff item", error));
        }
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
        Ok(())
    }

    /// Resolve the parent id for a canonical URL: reuse the existing parent
    /// (ADR-0013 §4.2), otherwise mint a fresh UUID v4 from the OS entropy
    /// seam, shaped like the Home identity ids.
    fn remote_id_for(&self, canonical_url: &str) -> Result<String, AdoptError> {
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

    fn block_for_recovery(&self, context: &str, error: impl std::fmt::Display) -> AdoptError {
        self.write_gate.mark_blocked();
        AdoptError::RecoveryRequired(format!("{context}: {error}"))
    }

    /// Move a migrated entity back to its real-directory appearance and
    /// recreate every symlink appearance (the entries were freed by removing
    /// the Activations first).
    fn restore_entity_and_appearances(
        &self,
        item: &PlannedAdoptItem,
        journal_item: &AdoptJournalItem,
        operation_id: &str,
    ) -> Result<(), AdoptError> {
        if matches!(item.kind, AdoptPlanKind::Migrate) {
            match self.real_tree_snapshot(&item.canonical_entity)? {
                Some(snapshot) if snapshot.content_hash == item.source_snapshot.content_hash => {}
                Some(_) => {
                    return Err(AdoptError::RecoveryRequired(format!(
                        "the original Adopt source changed while recovery was required: {}",
                        item.canonical_entity.display()
                    )));
                }
                None => {
                    let source =
                        if let Some(snapshot) = self.real_tree_snapshot(&item.final_entity_path)? {
                            if snapshot.content_hash != item.source_snapshot.content_hash {
                                return Err(AdoptError::RecoveryRequired(format!(
                                    "the Library entity no longer matches the planned source: {}",
                                    item.final_entity_path.display()
                                )));
                            }
                            (&item.final_entity_path, snapshot.root)
                        } else if let Some(snapshot) = self.real_tree_snapshot(&item.staged_root)? {
                            if snapshot.content_hash != item.source_snapshot.content_hash {
                                return Err(AdoptError::RecoveryRequired(format!(
                                    "the staged entity no longer matches the planned source: {}",
                                    item.staged_root.display()
                                )));
                            }
                            (&item.staged_root, snapshot.root)
                        } else {
                            return Err(AdoptError::RecoveryRequired(format!(
                                "the original, staged, and Library copies are all missing: {}",
                                item.canonical_entity.display()
                            )));
                        };
                    self.filesystem.restore_external_directory(
                        source.0,
                        &item.canonical_entity,
                        &source.1,
                    )?;
                }
            }
        }
        for appearance in item.appearances.iter().rev() {
            match &appearance.kind {
                AdoptAppearanceKind::RealDirectory => {}
                AdoptAppearanceKind::Symlink { original_target } => {
                    match self
                        .filesystem
                        .activation_snapshot(&appearance.entry_path)?
                    {
                        ActivationEntrySnapshot::Missing => {
                            self.filesystem
                                .create_activation(original_target, &appearance.entry_path)?;
                        }
                        ActivationEntrySnapshot::Symlink { target }
                            if target == *original_target => {}
                        ActivationEntrySnapshot::Symlink { .. }
                        | ActivationEntrySnapshot::Other => {
                            return Err(AdoptError::RecoveryRequired(format!(
                                "the original Adopt appearance is occupied: {}",
                                appearance.entry_path.display()
                            )));
                        }
                    }
                }
            }
        }
        if let Some(source_fingerprint) = &journal_item.source_fingerprint {
            self.filesystem.discard_isolated_adopt_source(
                &item.canonical_entity,
                operation_id,
                source_fingerprint,
            )?;
        }
        Ok(())
    }

    fn real_tree_snapshot(&self, path: &Path) -> Result<Option<StagedTreeSnapshot>, AdoptError> {
        match self.filesystem.activation_snapshot(path)? {
            ActivationEntrySnapshot::Missing => Ok(None),
            ActivationEntrySnapshot::Symlink { .. } => Err(AdoptError::RecoveryRequired(format!(
                "the recovery path is occupied by a non-directory entry: {}",
                path.display()
            ))),
            ActivationEntrySnapshot::Other => match self.filesystem.staged_tree_snapshot(path) {
                Ok(snapshot) => Ok(Some(snapshot)),
                Err(FileSystemError::NotDirectory { .. }) => {
                    Err(AdoptError::RecoveryRequired(format!(
                        "the recovery path is occupied by a non-directory entry: {}",
                        path.display()
                    )))
                }
                Err(error) => Err(AdoptError::from(error)),
            },
        }
    }

    /// Undo a whole applied batch while its result window is open. Each Skill
    /// is restored in reverse order; an item whose original location is now
    /// occupied is skipped and reported, never overwritten.
    pub fn undo(&self, operation_id: &str) -> Result<AdoptUndoResult, AdoptError> {
        self.ensure_writes_ready()?;
        let mut batch = self
            .applied
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
            .remove(operation_id)
            .ok_or(AdoptError::PlanNotFound)?;
        // Successful items: Done adopt items and Finalized handoff items
        // (each tracked by its own journal).
        let mut successful_items = Vec::new();
        let mut handoff_index = 0_usize;
        for (index, item) in batch.items.iter().enumerate() {
            if item.handoff.is_some() {
                let done = batch.handoff_journal.as_ref().is_some_and(|journal| {
                    journal.items[handoff_index].phase == HandoffItemPhase::Finalized
                });
                if done {
                    successful_items.push((index, Some(handoff_index)));
                }
                handoff_index += 1;
            } else if item.journal.phase == AdoptItemPhase::Done {
                successful_items.push((index, None));
            }
        }
        let mut results = Vec::with_capacity(successful_items.len());
        let mut snapshot_version = 0_u64;
        let mut handoff_journal = batch.handoff_journal.clone();
        for (index, handoff_index) in successful_items.into_iter().rev() {
            let item = batch.items[index].clone();
            let preflight = match handoff_index {
                Some(handoff_index) => self.preflight_undo_handoff_item(
                    &item,
                    handoff_journal
                        .as_ref()
                        .expect("handoff journal present")
                        .items[handoff_index]
                        .clone(),
                ),
                None => self.preflight_undo_item(&item),
            };
            match preflight {
                Ok(()) => {}
                Err(AdoptError::Validation(error)) => {
                    results.push(AdoptUndoItemResult {
                        directory_name: item.directory_name,
                        undone: false,
                        error: Some(error),
                    });
                    continue;
                }
                Err(error) => {
                    return Err(self.block_for_recovery("preflight Adopt Undo", error));
                }
            }
            let undone = match handoff_index {
                Some(handoff_index) => self.undo_handoff_item(
                    handoff_journal.as_mut().expect("handoff journal present"),
                    handoff_index,
                    &item,
                ),
                None => self.undo_item(&mut batch.journal, index, &item),
            };
            match undone {
                Ok(version) => {
                    snapshot_version = version;
                    if handoff_index.is_none() {
                        batch.items[index].journal = batch.journal.items[index].clone();
                    }
                    results.push(AdoptUndoItemResult {
                        directory_name: item.directory_name.clone(),
                        undone: true,
                        error: None,
                    });
                }
                Err(error) => {
                    return Err(self.block_for_recovery(
                        &format!("undo Adopt item '{}'", item.directory_name),
                        error,
                    ));
                }
            }
        }
        if let Some(handoff_journal) = handoff_journal.as_ref() {
            // The Undo window closes: discard the isolation copies and
            // archive the journal.
            for item in &handoff_journal.items {
                if let Some(isolated) = &item.isolated_path {
                    if let Err(error) = self.filesystem.discard_isolated_source(isolated) {
                        return Err(self.block_for_recovery(
                            "discard handoff isolation copy after Undo",
                            error,
                        ));
                    }
                }
            }
            if let Err(error) = self
                .filesystem
                .finish_handoff_journal(&self.active_library_root()?, &handoff_journal.operation_id)
            {
                return Err(self.block_for_recovery("archive completed Handoff Undo", error));
            }
        }
        if !batch.journal.items.is_empty() {
            if let Err(error) = self
                .filesystem
                .finish_adopt_journal(&self.active_library_root()?, operation_id)
            {
                return Err(self.block_for_recovery("archive completed Adopt Undo", error));
            }
        }
        Ok(AdoptUndoResult {
            operation_id: operation_id.to_owned(),
            items: results,
            snapshot_version,
        })
    }

    /// Verify the whole item before removing any Activation. An occupied
    /// original path is a normal per-item skip; missing or changed managed
    /// content requires journal recovery and must stop the write session.
    fn preflight_undo_item(&self, item: &PlannedAdoptItem) -> Result<(), AdoptError> {
        if matches!(item.kind, AdoptPlanKind::Migrate) {
            let snapshot = self
                .real_tree_snapshot(&item.final_entity_path)?
                .ok_or_else(|| {
                    AdoptError::RecoveryRequired(format!(
                        "the Library entity is missing before Undo: {}",
                        item.final_entity_path.display()
                    ))
                })?;
            if snapshot.content_hash != item.source_snapshot.content_hash {
                return Err(AdoptError::RecoveryRequired(format!(
                    "the Library entity changed before Undo: {}",
                    item.final_entity_path.display()
                )));
            }
            if let Some(expected) = &item.journal.installed_fingerprint {
                if snapshot.root.device != expected.device || snapshot.root.inode != expected.inode
                {
                    return Err(AdoptError::RecoveryRequired(format!(
                        "the Library entity was replaced before Undo: {}",
                        item.final_entity_path.display()
                    )));
                }
            }
        }

        for activation in &item.activations {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(AdoptError::Validation(format!(
                        "the original Adopt appearance is occupied: {}",
                        activation.entry_path.display()
                    )));
                }
            }
        }

        for appearance in &item.appearances {
            let planned_target = item
                .activations
                .iter()
                .find(|activation| activation.entry_path == appearance.entry_path)
                .map(|activation| &activation.target_path);
            match (
                &appearance.kind,
                self.filesystem
                    .activation_snapshot(&appearance.entry_path)?,
            ) {
                (_, ActivationEntrySnapshot::Missing) => {}
                (
                    AdoptAppearanceKind::RealDirectory,
                    ActivationEntrySnapshot::Symlink { target },
                ) if planned_target.is_some_and(|planned| *planned == target) => {}
                (
                    AdoptAppearanceKind::Symlink { original_target },
                    ActivationEntrySnapshot::Symlink { target },
                ) if target == *original_target
                    || planned_target.is_some_and(|planned| *planned == target) => {}
                (
                    AdoptAppearanceKind::RealDirectory | AdoptAppearanceKind::Symlink { .. },
                    ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other,
                ) => {
                    return Err(AdoptError::Validation(format!(
                        "the original Adopt appearance is occupied: {}",
                        appearance.entry_path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// Conditional Undo guards for a handoff item (ADR-0013 §5.1): the
    /// managed entity and Activations must be unchanged, the external
    /// canonical location must be recoverable (absent), and for remote
    /// intents the lock must still parse strictly with the key unoccupied.
    fn preflight_undo_handoff_item(
        &self,
        item: &PlannedAdoptItem,
        journal_item: HandoffJournalItem,
    ) -> Result<(), AdoptError> {
        let is_home_install = journal_item.remote.is_some();
        if is_home_install {
            let snapshot = self
                .real_tree_snapshot(&item.final_entity_path)?
                .ok_or_else(|| {
                    AdoptError::RecoveryRequired(format!(
                        "the Home entity is missing before Undo: {}",
                        item.final_entity_path.display()
                    ))
                })?;
            if snapshot.content_hash != journal_item.current_baseline_hash {
                return Err(AdoptError::RecoveryRequired(format!(
                    "the Home entity changed before Undo: {}",
                    item.final_entity_path.display()
                )));
            }
        } else {
            let snapshot = self
                .real_tree_snapshot(&item.final_entity_path)?
                .ok_or_else(|| {
                    AdoptError::RecoveryRequired(format!(
                        "the Link entity is missing before Undo: {}",
                        item.final_entity_path.display()
                    ))
                })?;
            if snapshot.content_hash != journal_item.source_tree_hash {
                return Err(AdoptError::RecoveryRequired(format!(
                    "the Link entity changed before Undo: {}",
                    item.final_entity_path.display()
                )));
            }
        }
        match self
            .filesystem
            .activation_snapshot(&item.canonical_entity)?
        {
            ActivationEntrySnapshot::Missing => {}
            ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                return Err(AdoptError::Validation(format!(
                    "the external canonical location is occupied: {}",
                    item.canonical_entity.display()
                )));
            }
        }
        for activation in &item.activations {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(AdoptError::Validation(format!(
                        "the handoff Activation is occupied: {}",
                        activation.entry_path.display()
                    )));
                }
            }
        }
        if !journal_item.lock_path.as_os_str().is_empty() {
            let reports = self.lock_store.discover().map_err(AdoptError::Lock)?;
            // ADR-0013 §5.1: the lock must still be strictly valid and the
            // key unoccupied; a missing or faulted lock file is never a
            // valid basis for restoring the old entry.
            let Some(report) = reports
                .iter()
                .find(|report| report.path == journal_item.lock_path)
            else {
                return Err(AdoptError::Validation(
                    "the installer lock file is missing; Undo is refused".into(),
                ));
            };
            if report.fault.is_some()
                || report
                    .entries
                    .iter()
                    .any(|entry| entry.name == journal_item.lock_entry_name)
            {
                return Err(AdoptError::Validation(
                    "the installer lock changed; Undo is refused".into(),
                ));
            }
        }
        Ok(())
    }

    fn undo_item(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<u64, AdoptError> {
        for activation in item.activations.iter().rev() {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                    self.filesystem.remove_activation(&activation.entry_path)?;
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(AdoptError::RecoveryRequired(format!(
                        "the Adopt Activation changed during Undo: {}",
                        activation.entry_path.display()
                    )));
                }
            }
        }
        let version = self.store.remove_adopted_skill(&item.skill_id)?;
        self.restore_entity_and_appearances(item, &item.journal, &journal.operation_id)?;
        let entry = &mut journal.items[index];
        entry.installed_fingerprint = None;
        entry.phase = AdoptItemPhase::Planned;
        self.filesystem
            .write_adopt_journal(&self.active_library_root()?, journal)?;
        Ok(version)
    }

    /// Undo one committed handoff item while its result window is open:
    /// remove the Activations and Catalog rows (with last-child parent
    /// cleanup), return the managed entity to the external canonical
    /// location (from the isolation copy for Home installs, from the
    /// stable directory for Links), CAS-restore the exact lock entry, and
    /// reset the item to Planned. Any guard failure above refuses without
    /// overwriting external work.
    fn undo_handoff_item(
        &self,
        handoff_journal: &mut HandoffJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<u64, AdoptError> {
        let journal_item = handoff_journal.items[index].clone();
        let is_home_install = journal_item.remote.is_some();
        // Convert-to-Link released the lock too (only Local Link with Move
        // never touches it); its Undo must restore the exact entry.
        let releases_lock = !journal_item.lock_path.as_os_str().is_empty();
        for activation in item.activations.iter().rev() {
            match self
                .filesystem
                .activation_snapshot(&activation.entry_path)?
            {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                    self.filesystem.remove_activation(&activation.entry_path)?;
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(AdoptError::RecoveryRequired(format!(
                        "the handoff Activation changed during Undo: {}",
                        activation.entry_path.display()
                    )));
                }
            }
        }
        let version = self.store.remove_adopted_skill(&item.skill_id)?;
        if let Some(remote_id) = &journal_item.remote_id {
            self.store.delete_remote_parent_if_last_child(remote_id)?;
            self.filesystem.remove_remote_parent_manifest(
                &self.active_library_root()?.join("remotes"),
                remote_id,
            )?;
        }
        if is_home_install {
            self.filesystem
                .remove_directory_verified(&item.final_entity_path)?;
        } else {
            self.filesystem.restore_isolated_source(
                &item.final_entity_path,
                &item.canonical_entity,
                &journal_item.source_tree_hash,
            )?;
        }
        if is_home_install {
            if let Some(isolated) = &journal_item.isolated_path {
                self.filesystem.restore_isolated_source(
                    isolated,
                    &item.canonical_entity,
                    &journal_item.source_tree_hash,
                )?;
            }
        }
        if releases_lock {
            let entry: LockEntry =
                serde_json::from_str(&journal_item.lock_entry_json).map_err(|error| {
                    AdoptError::Internal(format!(
                        "the frozen lock entry cannot be restored: {error}"
                    ))
                })?;
            self.lock_store
                .restore_entry(&journal_item.lock_path, &entry)?;
        }
        handoff_journal.items[index].isolated_path = None;
        handoff_journal.items[index].phase = HandoffItemPhase::Planned;
        self.filesystem
            .write_handoff_journal(&self.active_library_root()?, handoff_journal)?;
        Ok(version)
    }

    /// Close the result window: archive the journal and drop the batch, so
    /// the batch can no longer be undone.
    pub fn finalize(&self, operation_id: &str) -> Result<(), AdoptError> {
        self.ensure_writes_ready()?;
        let batch = self
            .applied
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
            .remove(operation_id);
        let Some(batch) = batch else {
            // The batch was already dropped (a second finalize is a
            // no-op); nothing left to archive.
            return Ok(());
        };
        if !batch.journal.items.is_empty() {
            self.filesystem
                .finish_adopt_journal(&self.active_library_root()?, operation_id)?;
        }
        if let Some(handoff_journal) = &batch.handoff_journal {
            // The result window closes: discard the isolation copies and
            // archive the journal.
            for item in &handoff_journal.items {
                if let Some(isolated) = &item.isolated_path {
                    if let Err(error) = self.filesystem.discard_isolated_source(isolated) {
                        return Err(self.block_for_recovery(
                            "discard handoff isolation copy on finalize",
                            error,
                        ));
                    }
                }
            }
            self.filesystem.finish_handoff_journal(
                &self.active_library_root()?,
                &handoff_journal.operation_id,
            )?;
        }
        Ok(())
    }

    fn ensure_writes_ready(&self) -> Result<(), AdoptError> {
        if self.write_gate.is_product_write_open() {
            Ok(())
        } else {
            Err(AdoptError::RecoveryRequired(
                "startup recovery is still in progress".into(),
            ))
        }
    }
}

fn adopt_validation(error: crate::core::import::ImportError) -> AdoptError {
    AdoptError::Validation(error.to_string())
}
