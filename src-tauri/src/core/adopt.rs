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

use crate::core::domain::{AgentId, AgentKind, SkillId, parse_skill_metadata};
use crate::core::import::LibraryConflict;
use crate::core::write_gate::{PlanCheck, PlanTicket, WriteGate};
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedActivation, AdoptedSkillRecord,
};
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, AdoptActivationStep, AdoptAppearanceKind as JournalAppearanceKind,
    AdoptAppearanceStep, AdoptItemPhase, AdoptJournal, AdoptJournalItem, AdoptJournalKind,
    AdoptJournalPhase, DirectoryFingerprint, FileSystem, FileSystemError, StagedTreeSnapshot,
};
use crate::seams::installer_lock_store::{
    EmptyInstallerLockStore, InstallerLockError, InstallerLockStore,
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
    pub shared: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptPlanKind {
    /// The entity is moved into the Library (file Install).
    Migrate,
    /// The entity stays outside; the Library records a pointer (Link).
    Link,
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
    #[error("Adopt requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Store(#[from] AdoptStoreError),
    #[error(transparent)]
    Lock(#[from] InstallerLockError),
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
}

#[derive(Clone)]
struct PlannedAdoptBatch {
    operation_id: String,
    items: Vec<PlannedAdoptItem>,
    journal: AdoptJournal,
    gate_generation: u64,
    created_at_millis: u128,
}

pub struct AdoptService {
    store: Arc<dyn AdoptStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    lock_store: Arc<dyn InstallerLockStore>,
    remote_provider: Arc<dyn RemoteProvider>,
    library_root: PathBuf,
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
            library_root,
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

    pub fn with_lock_store(mut self, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        self.lock_store = lock_store;
        self
    }

    pub fn with_remote_provider(mut self, remote_provider: Arc<dyn RemoteProvider>) -> Self {
        self.remote_provider = remote_provider;
        self
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
        let plan_number = plan_token
            .strip_prefix("adopt-plan-")
            .unwrap_or("0");
        let operation_id = format!(
            "adopt-{}-{plan_number}",
            self.clock.unix_epoch_nanos()
        );
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        let mut planned_items = Vec::with_capacity(items.len());
        for item in items {
            let mut activations = Vec::new();
            for appearance in &item.appearances {
                if appearance.shared {
                    continue;
                }
                let agent_id = appearance.agent_id.clone().ok_or_else(|| {
                    AdoptError::Internal("Agent appearance lacks an Agent".into())
                })?;
                activations.push(AdoptActivationStep {
                    agent_id: agent_id.0,
                    // The scan already returned absolute entry paths; they must
                    // stay unresolved (the entry IS the symlink being replaced).
                    entry_path: appearance.entry_path.clone(),
                    target_path: item.final_entity_path.clone(),
                });
            }
            for agent in agents.iter().filter(|agent| {
                item.target_agents
                    .iter()
                    .any(|target| target.agent_id == agent.agent_id)
            }) {
                activations.push(AdoptActivationStep {
                    agent_id: agent.agent_id.0.clone(),
                    entry_path: self
                        .filesystem
                        .normalize_configured_path(&agent.skills_path.join(&item.directory_name))?,
                    target_path: item.final_entity_path.clone(),
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
            let required_space = if matches!(item.kind, AdoptPlanKind::Migrate) {
                source_snapshot
                    .total_file_bytes
                    .saturating_mul(2)
                    .saturating_add(DISK_SPACE_RESERVE_BYTES)
            } else {
                0
            };
            if required_space > 0 {
                let available_space = self.filesystem.available_space(&self.library_root)?;
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
            let recorded_content_hash = if matches!(item.kind, AdoptPlanKind::Migrate) {
                source_snapshot.content_hash.clone()
            } else {
                String::new()
            };
            let staged_root = if matches!(item.kind, AdoptPlanKind::Migrate) {
                staging_operation_root.join(&item.directory_name)
            } else {
                PathBuf::new()
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
            planned_items.push(PlannedAdoptItem {
                skill_id: skill_id.clone(),
                directory_name: item.directory_name.clone(),
                identity_key,
                display_name: metadata
                    .name
                    .clone()
                    .unwrap_or_else(|| item.directory_name.clone()),
                description: metadata.description.unwrap_or_default(),
                kind: item.kind,
                canonical_entity: item.canonical_entity.clone(),
                final_entity_path: item.final_entity_path.clone(),
                staged_root,
                source_snapshot,
                staged_snapshot: None,
                appearances: item.appearances.clone(),
                activations,
                journal,
            });
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
            items: planned_items
                .iter()
                .map(|item| item.journal.clone())
                .collect(),
        };
        let batch = PlannedAdoptBatch {
            operation_id,
            items: planned_items,
            journal,
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
                    if matches!(item.intent, AdoptPlanIntent::LocalLink) {
                        self.recheck_frozen_evidence(&item.frozen)?;
                    }
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
        journal.phase = AdoptJournalPhase::Applying;
        if let Err(error) = self
            .filesystem
            .write_adopt_journal(&self.library_root, &journal)
        {
            return Err(self.block_for_recovery("persist Adopt intent", error));
        }
        journal.staging_fingerprint = match self
            .filesystem
            .create_adopt_staging_operation(&self.library_root, &journal.operation_id)
        {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                return Err(self.block_for_recovery("prepare Adopt staging", error));
            }
        };
        if let Err(error) = self
            .filesystem
            .write_adopt_journal(&self.library_root, &journal)
        {
            return Err(self.block_for_recovery("persist Adopt staging intent", error));
        }
        let mut results = Vec::with_capacity(batch.items.len());
        let mut snapshot_version = 0_u64;
        for index in 0..batch.items.len() {
            let outcome = {
                let item = &mut batch.items[index];
                self.prepare_item_for_apply(&mut journal, index, item)
                    .and_then(|()| self.apply_item(&mut journal, index, item))
            };
            let item = &batch.items[index];
            match outcome {
                Ok(version) => {
                    snapshot_version = version;
                    results.push(AdoptSkillResult {
                        skill_id: item.skill_id.clone(),
                        directory_name: item.directory_name.clone(),
                        adopted: true,
                        error: None,
                    });
                }
                Err(error) => {
                    let rollback = self.rollback_item(&mut journal, index, item);
                    match rollback {
                        Ok(()) => results.push(AdoptSkillResult {
                            skill_id: item.skill_id.clone(),
                            directory_name: item.directory_name.clone(),
                            adopted: false,
                            error: Some(error.to_string()),
                        }),
                        Err(compensation) => {
                            return Err(self.block_for_recovery(
                                "roll back failed Adopt item",
                                format!("{error}; rollback also failed: {compensation}"),
                            ));
                        }
                    }
                }
            }
            batch.items[index].journal = journal.items[index].clone();
        }
        if let Err(error) = self.filesystem.discard_staging(
            &journal.staging_operation_root,
            &self.library_root,
            Some(&journal.staging_fingerprint),
        ) {
            return Err(self.block_for_recovery("clean Adopt staging after Apply", error));
        }
        journal.phase = AdoptJournalPhase::Committed;
        if let Err(error) = self
            .filesystem
            .write_adopt_journal(&self.library_root, &journal)
        {
            return Err(self.block_for_recovery("persist committed Adopt batch", error));
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
        } else if let Err(error) = self
            .filesystem
            .finish_adopt_journal(&self.library_root, &operation_id)
        {
            return Err(self.block_for_recovery("archive empty Adopt batch", error));
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
                .write_adopt_journal(&self.library_root, journal)?;
            return Ok(());
        }

        let staged_fingerprint = self
            .filesystem
            .stage_external_directory_in_adopt_operation(
                &item.canonical_entity,
                &self.library_root,
                &journal.operation_id,
                &item.directory_name,
                &journal.staging_fingerprint,
                &item.source_snapshot.root,
            )?;
        journal.items[index].staged_fingerprint = staged_fingerprint;
        journal.items[index].phase = AdoptItemPhase::Staged;
        self.filesystem
            .write_adopt_journal(&self.library_root, journal)?;

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
                &self.library_root,
                &journal.operation_id,
                snapshot,
            )?;
            let entry = &mut journal.items[index];
            entry.installed_fingerprint = Some(fingerprint);
            entry.phase = AdoptItemPhase::EntityInstalled;
            self.filesystem
                .write_adopt_journal(&self.library_root, journal)?;
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
                    agent_id: AgentId(activation.agent_id.clone()),
                    expected_entry_path: activation.entry_path.clone(),
                    expected_target_path: activation.target_path.clone(),
                })
                .collect(),
        };
        let snapshot_version = self.store.insert_adopted(record)?;
        journal.items[index].phase = AdoptItemPhase::CatalogCommitted;
        self.filesystem
            .write_adopt_journal(&self.library_root, journal)?;
        self.filesystem
            .apply_adopt_appearances(&journal.items[index].appearances, &item.activations)?;
        journal.items[index].phase = AdoptItemPhase::Done;
        self.filesystem
            .write_adopt_journal(&self.library_root, journal)?;
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
            .write_adopt_journal(&self.library_root, journal)?;
        Ok(())
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
        let successful_items = batch
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                (item.journal.phase == AdoptItemPhase::Done).then_some(index)
            })
            .collect::<Vec<_>>();
        let mut results = Vec::with_capacity(successful_items.len());
        let mut snapshot_version = 0_u64;
        for index in successful_items.into_iter().rev() {
            let item = batch.items[index].clone();
            match self.preflight_undo_item(&item) {
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
            match self.undo_item(&mut batch.journal, index, &item) {
                Ok(version) => {
                    snapshot_version = version;
                    batch.items[index].journal = batch.journal.items[index].clone();
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
        if let Err(error) = self
            .filesystem
            .finish_adopt_journal(&self.library_root, operation_id)
        {
            return Err(self.block_for_recovery("archive completed Adopt Undo", error));
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
            .write_adopt_journal(&self.library_root, journal)?;
        Ok(version)
    }

    /// Close the result window: archive the journal and drop the batch, so
    /// the batch can no longer be undone.
    pub fn finalize(&self, operation_id: &str) -> Result<(), AdoptError> {
        self.ensure_writes_ready()?;
        self.applied
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
            .remove(operation_id);
        self.filesystem
            .finish_adopt_journal(&self.library_root, operation_id)?;
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


