//! Enable Module (spec §4.9; ADR-0019): the single Core-owned vertical
//! slice for Global Activation. React never composes per-Skill commands;
//! Core resolves Agent Configurations to canonical Target groups and
//! commits every `(Skill, Target, Directory Identity)` cell through one
//! journaled plan → apply → undo/finalize state machine.
//!
//! - `list_target_groups` resolves the current Skill's canonical Target
//!   groups (every referencing Agent, path, desired/observed state,
//!   compatibility and typed availability). A Target that is absent or
//!   unverifiable only yields `open_agent_management`; nothing is created.
//! - `plan_global_enable` / `plan_global_lifecycle` freeze the write-gate,
//!   catalog and Agent generations, the Source Snapshot health gate, the
//!   Target identity, the entry `lstat` and the occupier ownership; apply
//!   re-verifies all of it (any change → `PlanStale`).
//! - Apply commits cells in Target-selection order (chosen group order,
//!   then requested Skill order). Ordinary cell failures are isolated; a
//!   WriteGate/Home-identity break marks the remaining cells
//!   `not_attempted` and stops.
//! - Occupier rules (ADR-0019): Managed → Switch on this Target only;
//!   Untracked single-Skill → Adopt / Remove-then-replace / Cancel; an
//!   untracked exact-direct global link is backed up and rebuilt; a real
//!   directory is previewed with its file/directory counts, renamed into
//!   the operation backup and never recursively deleted.
//! - Every successful cell keeps before/after CAS facts; `undo` re-verifies
//!   each cell and rejects only externally changed ones; `finalize` or a
//!   restart cleans identity-verified committed backups and ends the Undo
//!   window.
//!
//! Project Enable (#89) reuses the same journal machinery; this module owns
//! the Global surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::{ActivationObservedState, Compatibility, Health, SkillId, SourceKind};
use crate::core::source_update::SourceUpdateError;
use crate::core::source_update::SourceUpdateService;
use crate::core::write_gate::{
    HomeWriteContext, PlanCheck, PlanTicket, ProductWriteGuard, WriteGate, WriteGateError,
};
use crate::seams::activation_store::{
    ActivationCellRow, ActivationCellWrite, ActivationStore, ActivationStoreError,
};
use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootProbe,
};
use crate::seams::agent_configuration_store::RecentProjectFolder;
use crate::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreError, AgentConfigurationStoreSnapshot,
    StoredAgentConfiguration, StoredGlobalSkillRoot,
};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, ActivationReplacePhase, DirectoryFingerprint, EnableCellAction,
    EnableJournal, EnableJournalCell, EvidenceChainHop, FileSystem, FileSystemError, OccupantKind,
    OccupantSnapshot, ProjectTargetFault, ProjectTargetResolution, StagedEntryKind,
};

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);
const JOURNAL_VERSION: u32 = 1;

/// The closed Global lifecycle actions (spec §4.9 `action`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnableAction {
    Enable,
    Disable,
    Repair,
    Switch,
}

/// Typed Target availability (spec §4.9): a Target that is absent or cannot
/// be verified only routes to Agent Management; the Enable module never
/// creates or repairs Target directories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetGroupAvailability {
    Available,
    /// The root path does not exist (NotFound).
    Absent,
    /// Probe failure (permission/I/O/not a directory/dangling).
    Unavailable,
}

/// The only typed action a Target group exposes when it cannot support
/// Enable (spec §4.9: Target missing/mismatch → `open_agent_management`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetGroupAction {
    None,
    OpenAgentManagement,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetGroupMember {
    pub agent_id: String,
    pub agent_name: String,
    pub compatibility: Compatibility,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalTargetGroup {
    /// Canonical Target identity (the shared `root_id` after path-identity
    /// deduplication, ADR-0016).
    pub target_root_id: String,
    pub configured_path: PathBuf,
    /// The Agent Configurations whose Activation Target is this group.
    pub consumers: Vec<TargetGroupMember>,
    pub availability: TargetGroupAvailability,
    /// Raw probe failure reason, never user copy.
    pub diagnostic: Option<String>,
    /// The current Skill's desired state in this group.
    pub desired: bool,
    /// The last Activation health observation for this `(Skill, Target)`.
    pub observed: Option<ActivationObservedState>,
    pub action: TargetGroupAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalTargetGroupSnapshot {
    pub skill_id: SkillId,
    pub skill_name: String,
    pub agent_generation: u64,
    pub groups: Vec<GlobalTargetGroup>,
}

/// What occupies the Activation entry (spec §7.5 occupier table).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Occupier {
    Empty,
    /// Another Managed Skill owns the `(Target, Directory Identity)` entry.
    Managed {
        skill_id: SkillId,
        directory_name: String,
    },
    /// Untracked content: a symlink, a real directory or a file.
    Untracked {
        kind: UntrackedOccupierKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UntrackedOccupierKind {
    /// Link to something other than the planned final entity.
    Symlink {
        target: PathBuf,
    },
    RealDirectory,
    File {
        length: u64,
    },
}

/// Preview counts of a real directory that Remove-then-replace will back up
/// (spec §7.5: directory/file counts, never a Replace-all or recursive
/// delete).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DestructiveCounts {
    pub directories: u64,
    pub files: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellEligibility {
    Ready,
    NoOp,
    Skipped,
    Conflict,
    Blocked,
}

/// Typed closed reasons (presentation maps these to message keys).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellBlockedReason {
    TargetAbsent,
    TargetUnavailable,
    SourceSnapshotMismatch,
    TombstonedMember,
    EntityBroken,
    EntryOccupied,
    OutsideProjectRoot,
    SymlinkCycle,
    HopLimitExceeded,
    TargetNotDirectory,
}

/// The per-cell user decision for a Conflict (spec §7.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellResolution {
    /// Managed occupier: ownership Switch on this Target only.
    Switch,
    /// Untracked occupier: Remove then replace (backup + rebuild).
    Replace,
    /// Route to the existing Adopt flow; the cell stays Skipped here.
    Adopt,
    /// Leave the cell Skipped.
    Skip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRootEvidence {
    pub canonical_path: PathBuf,
    pub identity: String,
    /// All configured project-directory walks frozen by this plan. This is
    /// evidence only; the apply path re-runs each walk before writing.
    pub hop_evidence: Vec<ProjectHopEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectHopEvidence {
    pub agent_id: String,
    pub agent_name: String,
    pub configured_relative_path: PathBuf,
    pub resolved_container: PathBuf,
    pub hops: Vec<EvidenceChainHop>,
}

/// One plan cell: the `(Skill, Target, Directory Identity)` identity plus
/// its observed occupancy, eligibility and the user's resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableCell {
    /// Stable `"<skill_id>|<target_root_id>"` key (spec §4.9 cell identity).
    pub cell_key: String,
    pub skill_id: SkillId,
    pub skill_name: String,
    pub directory_name: String,
    pub directory_identity_key: String,
    pub target_root_id: String,
    pub target_path: PathBuf,
    pub entry_path: PathBuf,
    pub final_entity_path: PathBuf,
    pub action: EnableAction,
    /// The Agents (one per consumer) this cell affects.
    pub affected_agent_ids: Vec<String>,
    pub affected_agent_names: Vec<String>,
    pub occupancy: Occupier,
    /// Untracked one-hop link that already points at the final entity: never
    /// treated as no-op, still backed up and rebuilt (ADR-0019).
    pub occ_exact_direct: bool,
    pub destructive: Option<DestructiveCounts>,
    pub eligibility: CellEligibility,
    pub blocked_reason: Option<CellBlockedReason>,
    pub resolution: CellResolution,
    /// Raw blocked/conflict detail, never user copy (e.g. probe diagnostic).
    pub detail: Option<String>,
    pub create_steps: Vec<PathBuf>,
    pub hop_evidence: Vec<ProjectHopEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnablePlan {
    pub plan_token: String,
    pub scope: &'static str,
    pub write_gate_generation: u64,
    pub catalog_generation: u64,
    pub agent_generation: u64,
    pub project_root: Option<ProjectRootEvidence>,
    pub cells: Vec<EnableCell>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellOutcome {
    Succeeded,
    NoOp,
    Skipped,
    Failed,
    NotAttempted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableCellResult {
    pub cell_key: String,
    pub skill_id: SkillId,
    pub target_root_id: String,
    pub outcome: CellOutcome,
    /// Raw failure diagnostic, never user copy.
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableResult {
    pub operation_id: String,
    pub cells: Vec<EnableCellResult>,
    pub snapshot_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableUndoCellResult {
    pub cell_key: String,
    pub undone: bool,
    /// Raw rejection diagnostic, never user copy.
    pub diagnostic: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableUndoResult {
    pub operation_id: String,
    pub cells: Vec<EnableUndoCellResult>,
    pub snapshot_version: u64,
}

#[derive(Debug, Error)]
pub enum EnableError {
    #[error("{0}")]
    Validation(String),
    #[error("the Skill '{skill_id}' was not found in the Catalog")]
    SkillNotFound { skill_id: String },
    #[error("the Enable plan is stale")]
    PlanStale,
    #[error("the Enable plan was not found or expired")]
    PlanNotFound,
    #[error("the write gate is closed for product writes")]
    WriteGateClosed,
    #[error("the Git Source Member snapshots do not match the current Source Release")]
    SourceSnapshotMismatch,
    #[error("the Git Source Member is tombstoned; it cannot be enabled")]
    TombstonedMember,
    /// This cell only: its entry/occupancy/catalog state was externally
    /// changed between preflight and commit. The cell is rolled back and
    /// the remaining cells continue (spec §4.9 isolation).
    #[error("the cell was externally changed before it committed: {0}")]
    CellConflict(String),
    /// The operation cannot compensate; the write session must recover.
    #[error("Enable requires recovery: {0}")]
    RecoveryRequired(String),
    #[error(transparent)]
    Store(#[from] ActivationStoreError),
    #[error(transparent)]
    AgentStore(#[from] AgentConfigurationStoreError),
    #[error(transparent)]
    Catalog(#[from] CatalogStoreError),
    #[error(transparent)]
    AgentFileSystem(#[from] AgentConfigurationFileSystemError),
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error("internal Enable error: {0}")]
    Internal(String),
}

/// Frozen evidence of one planned cell; apply re-verifies every field before
/// the first filesystem mutation (spec §4.9: any change → PlanStale).
#[derive(Clone)]
struct PlannedCell {
    cell: EnableCell,
    /// Entry condition at plan time (identity of the occupier, if any).
    frozen_entry: Option<OccupantSnapshot>,
    frozen_health: Health,
    frozen_source_kind: SourceKind,
    frozen_final_entity: PathBuf,
    frozen_availability: TargetGroupAvailability,
    frozen_target: Option<DirectoryFingerprint>,
    frozen_project_targets: Vec<FrozenProjectTarget>,
    before_desired: bool,
}

#[derive(Clone)]
struct FrozenProjectTarget {
    configured_relative_path: PathBuf,
    resolution: ProjectTargetResolution,
}

#[derive(Clone)]
struct PlannedBatch {
    operation_id: String,
    scope: &'static str,
    canonical_project_root: Option<PathBuf>,
    write_context: HomeWriteContext,
    gate_generation: u64,
    catalog_generation: u64,
    agent_generation: u64,
    project_root_identity: Option<DirectoryFingerprint>,
    created_at_millis: u128,
    cells: Vec<PlannedCell>,
}

pub struct EnableService {
    store: Arc<dyn ActivationStore>,
    catalog: Arc<dyn CatalogStore>,
    agent_store: Arc<dyn AgentConfigurationStore>,
    agent_fs: Arc<dyn AgentConfigurationFileSystem>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    /// Spec §8.3 Source Snapshot gate; `None` fails open in test
    /// compositions without Git sources (production always provides it).
    source_update: Option<Arc<SourceUpdateService>>,
    /// Production reads the current verified Home through this context;
    /// standalone test compositions keep their explicit construction root.
    home_context: Option<Arc<WriteGate>>,
    configured_library_root: PathBuf,
    plans: Mutex<HashMap<String, PlannedBatch>>,
    applied: Mutex<HashMap<String, PlannedBatch>>,
    next_plan_id: AtomicU64,
    plan_ttl: Duration,
    write_gate: Arc<WriteGate>,
}

impl EnableService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn ActivationStore>,
        catalog: Arc<dyn CatalogStore>,
        agent_store: Arc<dyn AgentConfigurationStore>,
        agent_fs: Arc<dyn AgentConfigurationFileSystem>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        library_root: PathBuf,
        write_gate: Arc<WriteGate>,
    ) -> Self {
        Self {
            store,
            catalog,
            agent_store,
            agent_fs,
            filesystem,
            clock,
            source_update: None,
            home_context: None,
            configured_library_root: library_root,
            plans: Mutex::new(HashMap::new()),
            applied: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
            write_gate,
        }
    }

    /// Make Home-scoped plans resolve their paths from the bootstrap-verified
    /// Home rather than from the process's startup configuration.
    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.home_context = Some(home_context.clone());
        self.write_gate = home_context;
        self
    }

    /// The Source Snapshot gate (spec §8.3): `ensure_new_enable_allowed`
    /// blocks new Enable and Repair while a Git member snapshot mismatches.
    pub fn with_source_update(mut self, source_update: Arc<SourceUpdateService>) -> Self {
        self.source_update = Some(source_update);
        self
    }

    fn ensure_writes_ready(&self) -> Result<(), EnableError> {
        if !self.write_gate.is_product_write_open() {
            return Err(EnableError::WriteGateClosed);
        }
        Ok(())
    }

    fn active_library_root(&self) -> Result<PathBuf, EnableError> {
        match &self.home_context {
            Some(context) => context
                .bound_home()
                .map(|home| home.path)
                .map_err(|error| EnableError::RecoveryRequired(error.to_string())),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn new_id(&self) -> Result<String, EnableError> {
        let millis = self.clock.monotonic_millis().to_string();
        Ok(format!(
            "enable-{}-{}",
            millis,
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn block_for_recovery(&self, context: &str, error: impl std::fmt::Display) -> EnableError {
        self.write_gate.mark_blocked();
        EnableError::RecoveryRequired(format!("{context}: {error}"))
    }

    fn capture_write_context(&self) -> Result<HomeWriteContext, EnableError> {
        self.write_gate
            .capture_open_context()
            .map_err(|_| EnableError::WriteGateClosed)
    }

    fn acquire_write_guard(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, EnableError> {
        self.write_gate
            .acquire_product_write(context)
            .map_err(|error| match error {
                WriteGateError::Stale => EnableError::PlanStale,
                WriteGateError::Closed => EnableError::WriteGateClosed,
                other => EnableError::RecoveryRequired(other.to_string()),
            })
    }

    fn library_root_for_context(&self, context: &HomeWriteContext) -> Result<PathBuf, EnableError> {
        if self.home_context.is_some() {
            Ok(context.home.path.clone())
        } else {
            self.active_library_root()
        }
    }

    // ------------------------------------------------------------------
    // list_target_groups
    // ------------------------------------------------------------------

    /// Resolve the current Skill's canonical Target groups (spec §4.9):
    /// keyed by canonical Target identity, carrying every referencing Agent,
    /// the configured path, this Skill's desired/observed state, the Agents'
    /// compatibility and the typed availability. Absent/unverifiable Targets
    /// only produce `open_agent_management`; nothing is created.
    pub fn list_target_groups(
        &self,
        skill_id: &SkillId,
    ) -> Result<GlobalTargetGroupSnapshot, EnableError> {
        let skill = self
            .catalog
            .inspect(skill_id)?
            .ok_or_else(|| EnableError::SkillNotFound {
                skill_id: skill_id.0.clone(),
            })?;
        let snapshot = self.agent_store.agent_configuration_snapshot()?;
        let agent_generation = snapshot.snapshot_version;
        let mut groups = Vec::new();
        for root in &snapshot.roots {
            let consumers = self.target_consumers(&snapshot, root);
            if consumers.is_empty() {
                continue;
            }
            let probe = self.agent_fs.probe_root(&root.configured_path)?;
            let (availability, diagnostic, action) = match &probe {
                AgentRootProbe::Present { .. } => (
                    TargetGroupAvailability::Available,
                    None,
                    TargetGroupAction::None,
                ),
                AgentRootProbe::Absent => (
                    TargetGroupAvailability::Absent,
                    None,
                    TargetGroupAction::OpenAgentManagement,
                ),
                AgentRootProbe::Unavailable { diagnostic } => (
                    TargetGroupAvailability::Unavailable,
                    Some(diagnostic.clone()),
                    TargetGroupAction::OpenAgentManagement,
                ),
            };
            let rows = self.store.activation_cells_for_skill(skill_id)?;
            let (desired, observed) = rows
                .iter()
                .find(|cell| cell.target_root_id == root.root_id)
                .map(|cell| (cell.desired_enabled, cell.observed_state))
                .unwrap_or((false, None));
            groups.push(GlobalTargetGroup {
                target_root_id: root.root_id.clone(),
                configured_path: root.configured_path.clone(),
                consumers,
                availability,
                diagnostic,
                desired,
                observed,
                action,
            });
        }
        Ok(GlobalTargetGroupSnapshot {
            skill_id: skill_id.clone(),
            skill_name: skill.summary.display_name,
            agent_generation,
            groups,
        })
    }

    /// The Agents whose Activation Target membership points at the root.
    fn target_consumers(
        &self,
        snapshot: &AgentConfigurationStoreSnapshot,
        root: &StoredGlobalSkillRoot,
    ) -> Vec<TargetGroupMember> {
        snapshot
            .configurations
            .iter()
            .filter(|configuration| references_target(configuration, root))
            .map(|configuration| TargetGroupMember {
                agent_id: configuration.agent_id.clone(),
                agent_name: configuration.name.clone(),
                compatibility: configuration.compatibility,
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Planning
    // ------------------------------------------------------------------

    /// Plan a Global Enable for one or more Skills onto one or more Target
    /// groups (spec §4.9). Cells come out in Target-selection order, then
    /// requested Skill order; unresolved Conflict cells stay Skipped.
    pub fn plan_global_enable(
        &self,
        skill_ids: &[SkillId],
        target_group_ids: &[String],
        cell_resolutions: &[(String, CellResolution)],
    ) -> Result<EnablePlan, EnableError> {
        let write_context = self.capture_write_context()?;
        if skill_ids.is_empty() {
            return Err(EnableError::Validation(
                "at least one Skill is required for Global Enable".into(),
            ));
        }
        if target_group_ids.is_empty() {
            return Err(EnableError::Validation(
                "at least one Target group is required for Global Enable".into(),
            ));
        }
        let is_batch = skill_ids.len() > 1;
        let mut cells = Vec::new();
        for target_group_id in target_group_ids {
            for skill_id in skill_ids {
                cells.push(self.plan_cell(
                    skill_id,
                    target_group_id,
                    EnableAction::Enable,
                    Some(cell_resolutions),
                    is_batch,
                )?);
            }
        }
        resolve_batch_contention(&mut cells, cell_resolutions);
        self.build_plan(cells, write_context)
    }

    /// Plan one Global lifecycle action on one Target group (spec §4.9):
    /// Enable / Disable / Repair. Only one cell is produced.
    pub fn plan_global_lifecycle(
        &self,
        skill_id: &SkillId,
        target_group_id: &str,
        action: EnableAction,
    ) -> Result<EnablePlan, EnableError> {
        let write_context = self.capture_write_context()?;
        let cell = self.plan_cell(skill_id, target_group_id, action, None, false)?;
        self.build_plan(vec![cell], write_context)
    }

    /// Plan a Project Enable for one or more Skills in one project folder
    /// across one or more Agents (spec §4.9; ADR-0015; ADR-0019; #89).
    pub fn plan_project_enable(
        &self,
        skill_ids: &[SkillId],
        project_folder: &Path,
        agent_ids: &[String],
        cell_resolutions: &[(String, CellResolution)],
    ) -> Result<EnablePlan, EnableError> {
        let write_context = self.capture_write_context()?;
        if skill_ids.is_empty() {
            return Err(EnableError::Validation(
                "at least one Skill is required for Project Enable".into(),
            ));
        }
        if agent_ids.is_empty() {
            return Err(EnableError::Validation(
                "at least one Agent is required for Project Enable".into(),
            ));
        }
        let canonical_project_root = match self.filesystem.canonical_directory(project_folder) {
            Ok(path) => path,
            Err(error) => {
                return Err(EnableError::Validation(format!(
                    "the project folder '{}' cannot be canonicalized: {error}",
                    project_folder.display()
                )));
            }
        };

        let snapshot = self.agent_store.agent_configuration_snapshot()?;

        struct AgentResolution {
            agent_id: String,
            agent_name: String,
            configured_dir: PathBuf,
            resolution: ProjectTargetResolution,
            resolved_container: PathBuf,
            hops: Vec<EvidenceChainHop>,
            create_steps: Vec<PathBuf>,
            fault: Option<CellBlockedReason>,
            detail: Option<String>,
        }

        let mut resolved_agents = Vec::new();
        for agent_id in agent_ids {
            let agent = snapshot
                .configurations
                .iter()
                .find(|cfg| &cfg.agent_id == agent_id)
                .ok_or_else(|| {
                    EnableError::Validation(format!(
                        "Agent Configuration '{agent_id}' was not found"
                    ))
                })?;
            let configured_dir = match &agent.project_skills_dir {
                Some(dir) => dir.clone(),
                None => {
                    return Err(EnableError::Validation(format!(
                        "Agent '{}' has no project_skills_dir configured",
                        agent.name
                    )));
                }
            };
            let walked = self
                .filesystem
                .resolve_project_target(&canonical_project_root, &configured_dir)?;
            let (fault, detail) = match walked.fault.as_ref() {
                Some(ProjectTargetFault::OutsideProjectRoot) => {
                    (Some(CellBlockedReason::OutsideProjectRoot), None)
                }
                Some(ProjectTargetFault::SymlinkCycle) => {
                    (Some(CellBlockedReason::SymlinkCycle), None)
                }
                Some(ProjectTargetFault::HopLimitExceeded) => {
                    (Some(CellBlockedReason::HopLimitExceeded), None)
                }
                Some(ProjectTargetFault::TargetNotDirectory) => {
                    (Some(CellBlockedReason::TargetNotDirectory), None)
                }
                Some(ProjectTargetFault::TargetUnavailable { diagnostic }) => (
                    Some(CellBlockedReason::TargetUnavailable),
                    Some(diagnostic.clone()),
                ),
                None => (None, None),
            };
            resolved_agents.push(AgentResolution {
                agent_id: agent.agent_id.clone(),
                agent_name: agent.name.clone(),
                configured_dir,
                resolution: walked.clone(),
                resolved_container: walked.resolved_container,
                hops: walked.hops,
                create_steps: walked.create_steps,
                fault,
                detail,
            });
        }

        struct ResolvedGroup {
            target_root_id: String,
            resolved_container: PathBuf,
            affected_agent_ids: Vec<String>,
            affected_agent_names: Vec<String>,
            create_steps: Vec<PathBuf>,
            hop_evidences: Vec<ProjectHopEvidence>,
            target_resolutions: Vec<FrozenProjectTarget>,
            target_identity: Option<DirectoryFingerprint>,
            fault: Option<CellBlockedReason>,
            detail: Option<String>,
        }

        let mut groups: Vec<ResolvedGroup> = Vec::new();
        for resolved in resolved_agents {
            let hop_evidence = ProjectHopEvidence {
                agent_id: resolved.agent_id.clone(),
                agent_name: resolved.agent_name.clone(),
                configured_relative_path: resolved.configured_dir.clone(),
                resolved_container: resolved.resolved_container.clone(),
                hops: resolved.hops,
            };

            if let Some(existing) = groups
                .iter_mut()
                .find(|g| g.resolved_container == resolved.resolved_container)
            {
                existing.affected_agent_ids.push(resolved.agent_id);
                existing.affected_agent_names.push(resolved.agent_name);
                existing.hop_evidences.push(hop_evidence);
                existing.target_resolutions.push(FrozenProjectTarget {
                    configured_relative_path: resolved.configured_dir,
                    resolution: resolved.resolution,
                });
                for step in resolved.create_steps {
                    if !existing.create_steps.contains(&step) {
                        existing.create_steps.push(step);
                    }
                }
                if existing.fault.is_none() && resolved.fault.is_some() {
                    existing.fault = resolved.fault;
                    existing.detail = resolved.detail;
                }
            } else {
                let target_root_id =
                    format!("project:{}", resolved.resolved_container.to_string_lossy());
                groups.push(ResolvedGroup {
                    target_root_id,
                    resolved_container: resolved.resolved_container,
                    affected_agent_ids: vec![resolved.agent_id],
                    affected_agent_names: vec![resolved.agent_name],
                    create_steps: resolved.create_steps,
                    hop_evidences: vec![hop_evidence],
                    target_resolutions: vec![FrozenProjectTarget {
                        configured_relative_path: resolved.configured_dir,
                        resolution: resolved.resolution,
                    }],
                    target_identity: None,
                    fault: resolved.fault,
                    detail: resolved.detail,
                });
            }
        }

        for group in &mut groups {
            if group.fault.is_none() && group.create_steps.is_empty() {
                group.target_identity = Some(
                    self.filesystem
                        .directory_fingerprint(&group.resolved_container)
                        .map_err(|error| {
                            EnableError::Validation(format!(
                                "failed to fingerprint resolved project target '{}': {error}",
                                group.resolved_container.display()
                            ))
                        })?,
                );
            }
        }

        let mut cells = Vec::new();
        for group in &groups {
            for skill_id in skill_ids {
                let skill =
                    self.catalog
                        .inspect(skill_id)?
                        .ok_or_else(|| EnableError::SkillNotFound {
                            skill_id: skill_id.0.clone(),
                        })?;
                let directory_identity_key = self
                    .catalog
                    .skill_directory_identity_key(skill_id)?
                    .ok_or(EnableError::PlanStale)?;
                let final_entity = PathBuf::from(skill.final_entity_path.clone());
                let entry_path = group.resolved_container.join(&skill.summary.directory_name);
                let cell_key = format!("{}|{}", skill_id.0, group.target_root_id);

                let source_gate = self
                    .source_update
                    .as_ref()
                    .map(|service| map_gate_code(service.ensure_new_enable_allowed(skill_id)))
                    .unwrap_or(GateCode::Ok);

                let (
                    frozen_entry,
                    occupier,
                    exact_direct,
                    destructive,
                    eligibility,
                    blocked_reason,
                    resolution,
                    detail,
                ) = if let Some(fault) = group.fault {
                    (
                        None,
                        Occupier::Empty,
                        false,
                        None,
                        CellEligibility::Blocked,
                        Some(fault),
                        CellResolution::Skip,
                        group.detail.clone(),
                    )
                } else {
                    let (frozen_entry, occupier, exact_direct, destructive) =
                        self.observe_entry(&entry_path, &final_entity)?;

                    let (eligibility, blocked_reason, resolution, detail) =
                        if source_gate == GateCode::Mismatch {
                            (
                                CellEligibility::Blocked,
                                Some(CellBlockedReason::SourceSnapshotMismatch),
                                CellResolution::Skip,
                                None,
                            )
                        } else if source_gate == GateCode::Tombstoned {
                            (
                                CellEligibility::Blocked,
                                Some(CellBlockedReason::TombstonedMember),
                                CellResolution::Skip,
                                None,
                            )
                        } else if skill.summary.health == Health::Broken {
                            (
                                CellEligibility::Blocked,
                                Some(CellBlockedReason::EntityBroken),
                                CellResolution::Skip,
                                None,
                            )
                        } else if exact_direct {
                            (CellEligibility::NoOp, None, CellResolution::Skip, None)
                        } else if occupier == Occupier::Empty {
                            (CellEligibility::Ready, None, CellResolution::Replace, None)
                        } else {
                            let user_res = cell_resolutions
                                .iter()
                                .find(|(k, _)| k == &cell_key)
                                .map(|(_, r)| *r);
                            let res = match user_res {
                                Some(CellResolution::Replace) => CellResolution::Replace,
                                _ => CellResolution::Skip,
                            };
                            (CellEligibility::Conflict, None, res, None)
                        };
                    (
                        frozen_entry,
                        occupier,
                        exact_direct,
                        destructive,
                        eligibility,
                        blocked_reason,
                        resolution,
                        detail,
                    )
                };

                let cell = EnableCell {
                    cell_key,
                    skill_id: skill_id.clone(),
                    skill_name: skill.summary.display_name.clone(),
                    directory_name: skill.summary.directory_name.clone(),
                    directory_identity_key,
                    target_root_id: group.target_root_id.clone(),
                    target_path: group.resolved_container.clone(),
                    entry_path,
                    final_entity_path: final_entity.clone(),
                    action: EnableAction::Enable,
                    affected_agent_ids: group.affected_agent_ids.clone(),
                    affected_agent_names: group.affected_agent_names.clone(),
                    occupancy: occupier,
                    occ_exact_direct: exact_direct,
                    destructive,
                    eligibility,
                    blocked_reason,
                    resolution,
                    detail,
                    create_steps: group.create_steps.clone(),
                    hop_evidence: group.hop_evidences.clone(),
                };

                cells.push(PlannedCell {
                    cell,
                    frozen_entry,
                    frozen_health: skill.summary.health,
                    frozen_source_kind: skill.summary.source_kind,
                    frozen_final_entity: final_entity,
                    frozen_availability: TargetGroupAvailability::Available,
                    frozen_target: group.target_identity.clone(),
                    frozen_project_targets: group.target_resolutions.clone(),
                    before_desired: false,
                });
            }
        }

        resolve_batch_contention(&mut cells, cell_resolutions);
        let hop_evidence = groups
            .iter()
            .flat_map(|group| group.hop_evidences.clone())
            .collect::<Vec<_>>();
        self.build_project_plan(canonical_project_root, hop_evidence, cells, write_context)
    }

    fn build_project_plan(
        &self,
        canonical_project_root: PathBuf,
        hop_evidence: Vec<ProjectHopEvidence>,
        cells: Vec<PlannedCell>,
        write_context: HomeWriteContext,
    ) -> Result<EnablePlan, EnableError> {
        let plan_token = self.new_id()?;
        self.write_gate
            .validate_open_context(&write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => EnableError::PlanStale,
                WriteGateError::Closed => EnableError::WriteGateClosed,
                other => EnableError::RecoveryRequired(other.to_string()),
            })?;
        let gate_generation = write_context.generation;
        let catalog_generation = self.store.catalog_generation()?;
        let agent_generation = self
            .agent_store
            .agent_configuration_snapshot()?
            .snapshot_version;
        let fp = self
            .filesystem
            .directory_fingerprint(&canonical_project_root)
            .map_err(|e| {
                EnableError::Validation(format!("failed to inspect canonical project root: {e}"))
            })?;
        let identity = format!("{}:{}", fp.device, fp.inode);
        let project_root = ProjectRootEvidence {
            canonical_path: canonical_project_root.clone(),
            identity,
            hop_evidence,
        };
        let batch = PlannedBatch {
            operation_id: self.new_id()?,
            scope: "project",
            canonical_project_root: Some(canonical_project_root),
            write_context,
            gate_generation,
            catalog_generation,
            agent_generation,
            project_root_identity: Some(fp),
            created_at_millis: self.clock.monotonic_millis(),
            cells,
        };
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| EnableError::Internal("Enable plan lock poisoned".into()))?;
        plans.insert(plan_token.clone(), batch.clone());
        Ok(EnablePlan {
            plan_token,
            scope: "project",
            write_gate_generation: gate_generation,
            catalog_generation,
            agent_generation,
            project_root: Some(project_root),
            cells: batch
                .cells
                .iter()
                .map(|planned| planned.cell.clone())
                .collect(),
        })
    }

    fn build_plan(
        &self,
        cells: Vec<PlannedCell>,
        write_context: HomeWriteContext,
    ) -> Result<EnablePlan, EnableError> {
        let plan_token = self.new_id()?;
        self.write_gate
            .validate_open_context(&write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => EnableError::PlanStale,
                WriteGateError::Closed => EnableError::WriteGateClosed,
                other => EnableError::RecoveryRequired(other.to_string()),
            })?;
        let gate_generation = write_context.generation;
        let catalog_generation = self.store.catalog_generation()?;
        let agent_generation = self
            .agent_store
            .agent_configuration_snapshot()?
            .snapshot_version;
        let batch = PlannedBatch {
            operation_id: self.new_id()?,
            scope: "global",
            canonical_project_root: None,
            write_context,
            gate_generation,
            catalog_generation,
            agent_generation,
            project_root_identity: None,
            created_at_millis: self.clock.monotonic_millis(),
            cells,
        };
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| EnableError::Internal("Enable plan lock poisoned".into()))?;
        plans.insert(plan_token.clone(), batch.clone());
        Ok(EnablePlan {
            plan_token,
            scope: "global",
            write_gate_generation: gate_generation,
            catalog_generation,
            agent_generation,
            project_root: None,
            cells: batch
                .cells
                .iter()
                .map(|planned| planned.cell.clone())
                .collect(),
        })
    }

    /// Build one cell's facts: skill row, canonical Target, entry occupancy,
    /// eligibility, blocked reason and the frozen evidence.
    fn plan_cell(
        &self,
        skill_id: &SkillId,
        target_group_id: &str,
        action: EnableAction,
        resolutions: Option<&[(String, CellResolution)]>,
        is_batch: bool,
    ) -> Result<PlannedCell, EnableError> {
        let skill = self
            .catalog
            .inspect(skill_id)?
            .ok_or_else(|| EnableError::SkillNotFound {
                skill_id: skill_id.0.clone(),
            })?;
        let snapshot = self.agent_store.agent_configuration_snapshot()?;
        let root = snapshot
            .roots
            .iter()
            .find(|root| root.root_id == target_group_id)
            .ok_or_else(|| {
                EnableError::Validation(format!(
                    "the Target group '{target_group_id}' does not exist"
                ))
            })?;
        if !snapshot
            .configurations
            .iter()
            .any(|configuration| references_target(configuration, root))
        {
            return Err(EnableError::Validation(format!(
                "Global Enable target '{target_group_id}' is not an Agent Activation Target"
            )));
        }
        let probe = self.agent_fs.probe_root(&root.configured_path)?;
        let frozen_target = match &probe {
            AgentRootProbe::Present { canonical_path } => {
                let fingerprint = self
                    .filesystem
                    .directory_fingerprint(&root.configured_path)?;
                if fingerprint.canonical_path != canonical_path.clone() {
                    return Err(EnableError::PlanStale);
                }
                Some(fingerprint)
            }
            AgentRootProbe::Absent | AgentRootProbe::Unavailable { .. } => None,
        };
        let target_path = frozen_target
            .as_ref()
            .map(|target| target.canonical_path.clone())
            .unwrap_or_else(|| root.configured_path.clone());
        let entry_path = target_path.join(&skill.summary.directory_name);
        let final_entity = PathBuf::from(skill.final_entity_path.clone());
        let cell_key = format!("{}|{}", skill_id.0, root.root_id);
        let directory_identity_key = self
            .catalog
            .skill_directory_identity_key(skill_id)?
            .ok_or(EnableError::PlanStale)?;

        let (frozen_entry, occupier, exact_direct, destructive) =
            self.observe_entry(&entry_path, &final_entity)?;
        let rows = self.store.activation_cells_for_skill(skill_id)?;
        let current_row = rows
            .iter()
            .find(|cell| cell.target_root_id == root.root_id)
            .cloned();
        let before_desired = current_row
            .as_ref()
            .map(|cell| cell.desired_enabled)
            .unwrap_or(false);

        // Source Snapshot gate (spec §8.3): new Enable, Repair and Switch
        // are blocked while the member snapshot mismatches; Disable stays
        // open.
        let source_gate = match action {
            EnableAction::Enable | EnableAction::Repair | EnableAction::Switch => self
                .source_update
                .as_ref()
                .map(|service| map_gate_code(service.ensure_new_enable_allowed(skill_id)))
                .unwrap_or(GateCode::Ok),
            EnableAction::Disable => GateCode::Ok,
        };

        let availability = match probe {
            AgentRootProbe::Present { .. } => TargetGroupAvailability::Available,
            AgentRootProbe::Absent => TargetGroupAvailability::Absent,
            AgentRootProbe::Unavailable { .. } => TargetGroupAvailability::Unavailable,
        };

        let entry_is_ours = matches!(
            &frozen_entry,
            Some(snapshot)
                if matches!(&snapshot.kind, OccupantKind::Symlink { target } if *target == final_entity)
        );
        let (eligibility, blocked_reason, resolution, detail) = classify_cell(
            action,
            &frozen_entry,
            &occupier,
            exact_direct,
            entry_is_ours,
            before_desired,
            skill.summary.health,
            source_gate,
            availability,
            current_row.as_ref(),
            &cell_key,
            resolutions.unwrap_or(&[]),
            is_batch,
        );

        let affected_agent_ids = snapshot
            .configurations
            .iter()
            .filter(|configuration| references_target(configuration, root))
            .map(|configuration| configuration.agent_id.clone())
            .collect::<Vec<_>>();
        let affected_agent_names = snapshot
            .configurations
            .iter()
            .filter(|configuration| references_target(configuration, root))
            .map(|configuration| configuration.name.clone())
            .collect();

        let cell = EnableCell {
            cell_key,
            skill_id: skill_id.clone(),
            skill_name: skill.summary.display_name.clone(),
            directory_name: skill.summary.directory_name.clone(),
            directory_identity_key: directory_identity_key.clone(),
            target_root_id: root.root_id.clone(),
            target_path,
            entry_path,
            final_entity_path: final_entity.clone(),
            action,
            affected_agent_ids,
            affected_agent_names,
            occupancy: occupier,
            occ_exact_direct: exact_direct,
            destructive,
            eligibility,
            blocked_reason,
            resolution,
            detail,
            create_steps: Vec::new(),
            hop_evidence: Vec::new(),
        };
        Ok(PlannedCell {
            frozen_entry,
            frozen_health: skill.summary.health,
            frozen_source_kind: skill.summary.source_kind,
            frozen_final_entity: final_entity,
            frozen_availability: availability,
            frozen_target,
            frozen_project_targets: Vec::new(),
            before_desired,
            cell,
        })
    }

    /// Entry `lstat` + occupier classification (spec §4.9 frozen evidence).
    /// A symlink pointing at the planned final entity is still recorded as
    /// Untracked (never claimed as no-op; ADR-0019).
    fn observe_entry(
        &self,
        entry_path: &Path,
        final_entity: &Path,
    ) -> Result<
        (
            Option<OccupantSnapshot>,
            Occupier,
            bool,
            Option<DestructiveCounts>,
        ),
        EnableError,
    > {
        match self.filesystem.activation_snapshot(entry_path)? {
            ActivationEntrySnapshot::Missing => Ok((None, Occupier::Empty, false, None)),
            ActivationEntrySnapshot::Symlink { target } => {
                let snapshot = self.filesystem.occupant_snapshot(entry_path)?;
                let exact_direct = target == final_entity;
                let managed = self
                    .store
                    .desired_activations()?
                    .into_iter()
                    .find(|activation| activation.expected_target_path == target);
                let occupier = match managed {
                    Some(activation) => Occupier::Managed {
                        skill_id: activation.skill_id,
                        directory_name: entry_path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    },
                    None => Occupier::Untracked {
                        kind: UntrackedOccupierKind::Symlink { target },
                    },
                };
                Ok((Some(snapshot), occupier, exact_direct, None))
            }
            ActivationEntrySnapshot::Other => {
                let snapshot = self.filesystem.occupant_snapshot(entry_path)?;
                let (occupier, destructive) = match &snapshot.kind {
                    OccupantKind::RealDirectory => (
                        Occupier::Untracked {
                            kind: UntrackedOccupierKind::RealDirectory,
                        },
                        Some(self.staged_directory_counts(entry_path)?),
                    ),
                    OccupantKind::File { length } => (
                        Occupier::Untracked {
                            kind: UntrackedOccupierKind::File { length: *length },
                        },
                        None,
                    ),
                    OccupantKind::Symlink { .. } => {
                        return Err(EnableError::Internal(
                            "symlink entry classified as Other".into(),
                        ));
                    }
                };
                Ok((Some(snapshot), occupier, false, destructive))
            }
        }
    }

    /// The destructive preview counts of a real directory occupier (files
    /// and directory entries, never a recursive delete).
    fn staged_directory_counts(&self, path: &Path) -> Result<DestructiveCounts, EnableError> {
        let snapshot = self.filesystem.staged_tree_snapshot(path)?;
        let mut directories = 0_u64;
        let mut files = 0_u64;
        for entry in &snapshot.entries {
            match entry.kind {
                StagedEntryKind::Directory => directories += 1,
                StagedEntryKind::File { .. } => files += 1,
                StagedEntryKind::Symlink { .. } => {}
            }
        }
        Ok(DestructiveCounts { directories, files })
    }

    // ------------------------------------------------------------------
    // Apply
    // ------------------------------------------------------------------

    /// Apply a plan: re-verify all frozen evidence first (any change →
    /// PlanStale before the first write), then commit each cell in Target
    /// selection order. Ordinary cell failures are isolated; a WriteGate /
    /// Home-identity break marks the rest `not_attempted` and stops.
    pub fn apply(&self, plan_token: &str) -> Result<EnableResult, EnableError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| EnableError::Internal("Enable plan lock poisoned".into()))?;
        let batch = plans.remove(plan_token).ok_or(EnableError::PlanNotFound)?;
        if self.write_gate.check_plan(PlanTicket {
            generation: batch.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(EnableError::PlanStale);
        }
        self.write_gate
            .validate_open_context(&batch.write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => EnableError::PlanStale,
                WriteGateError::Closed => EnableError::WriteGateClosed,
                other => EnableError::RecoveryRequired(other.to_string()),
            })?;
        drop(plans);
        if self
            .clock
            .monotonic_millis()
            .saturating_sub(batch.created_at_millis)
            >= self.plan_ttl.as_millis()
        {
            return Err(EnableError::PlanNotFound);
        }
        let library_root = self.library_root_for_context(&batch.write_context)?;
        let _write_guard = self.acquire_write_guard(&batch.write_context)?;
        self.verify_plan(&batch)?;
        let mut journal = self.build_journal(&batch, &library_root);
        let has_runnable = journal
            .cells
            .iter()
            .any(|cell| cell.phase == ActivationReplacePhase::Applying);
        if has_runnable {
            if let Err(error) = self
                .filesystem
                .write_enable_journal(&library_root, &journal)
            {
                return Err(self.block_for_recovery("persist Enable intent", error));
            }
        }

        let mut results = Vec::with_capacity(batch.cells.len());
        let mut snapshot_version = self.store.catalog_generation()?;
        let mut stopped = false;
        for index in 0..batch.cells.len() {
            if stopped {
                results.push(self.result_for(&batch.cells[index], CellOutcome::NotAttempted, None));
                continue;
            }
            let runnable = journal.cells[index].phase == ActivationReplacePhase::Applying;
            let mut cell_error: Option<EnableError> = None;
            let outcome = if !runnable {
                match batch.cells[index].cell.eligibility {
                    CellEligibility::NoOp => CellOutcome::NoOp,
                    _ => CellOutcome::Skipped,
                }
            } else {
                match self.apply_cell(
                    &batch.cells[index],
                    &mut journal,
                    index,
                    &library_root,
                    batch.scope,
                    batch.project_root_identity.as_ref(),
                ) {
                    Ok(version) => {
                        snapshot_version = version;
                        CellOutcome::Succeeded
                    }
                    Err(EnableError::PlanStale) => {
                        // The gate/CAS broke mid-apply: the remaining cells
                        // are not attempted and the loop stops.
                        stopped = true;
                        CellOutcome::Failed
                    }
                    Err(EnableError::WriteGateClosed) => {
                        stopped = true;
                        CellOutcome::Failed
                    }
                    Err(EnableError::CellConflict(_)) => {
                        // Ordinary per-cell failure: isolated and continue.
                        cell_error = Some(EnableError::CellConflict(
                            "the entry or occupier changed before the cell committed".into(),
                        ));
                        CellOutcome::Failed
                    }
                    Err(
                        error @ (EnableError::Store(_)
                        | EnableError::FileSystem(_)
                        | EnableError::AgentFileSystem(_)
                        | EnableError::AgentStore(_)
                        | EnableError::Catalog(_)),
                    ) => {
                        cell_error = Some(error);
                        CellOutcome::Failed
                    }
                    Err(EnableError::RecoveryRequired(_)) => {
                        return Err(EnableError::RecoveryRequired(
                            "the Enable operation requires startup recovery".into(),
                        ));
                    }
                    Err(enable_error) => {
                        return Err(enable_error);
                    }
                }
            };
            let diagnostic = if outcome == CellOutcome::Failed {
                Some(cell_error.map_or_else(
                    || "the cell could not be committed; the prior state was restored".to_owned(),
                    |error| error.to_string(),
                ))
            } else {
                None
            };
            results.push(self.result_for(&batch.cells[index], outcome, diagnostic));
        }

        let has_succeeded = results
            .iter()
            .any(|result| result.outcome == CellOutcome::Succeeded);
        let operation_id = batch.operation_id.clone();
        if has_succeeded {
            if batch.scope == "project" {
                if let Some(project_root) = &batch.canonical_project_root {
                    let now = crate::seams::clock::iso_timestamp(self.clock.unix_epoch_nanos());
                    let canonical_str = project_root.to_string_lossy().into_owned();
                    let key = crate::core::domain::configured_path_identity_key(&canonical_str);
                    let _ = self
                        .agent_store
                        .record_recent_project_folder(RecentProjectFolder {
                            canonical_path_key: key,
                            canonical_path: project_root.clone(),
                            last_used_at: now,
                        });
                }
            }
            journal.phase = ActivationReplacePhase::Committed;
            if let Err(error) = self
                .filesystem
                .write_enable_journal(&library_root, &journal)
            {
                return Err(self.block_for_recovery("persist committed Enable batch", error));
            }
            match self.applied.lock() {
                Ok(mut applied) => {
                    applied.insert(batch.operation_id.clone(), batch);
                }
                Err(_) => {
                    return Err(self.block_for_recovery(
                        "retain committed Enable batch for Undo",
                        "Enable applied lock poisoned",
                    ));
                }
            }
        } else if has_runnable {
            if let Err(error) = self
                .filesystem
                .finish_enable_journal(&library_root, &operation_id)
            {
                return Err(self.block_for_recovery("archive empty Enable batch", error));
            }
        }
        Ok(EnableResult {
            operation_id,
            cells: results,
            snapshot_version,
        })
    }

    fn result_for(
        &self,
        planned: &PlannedCell,
        outcome: CellOutcome,
        diagnostic: Option<String>,
    ) -> EnableCellResult {
        EnableCellResult {
            cell_key: planned.cell.cell_key.clone(),
            skill_id: planned.cell.skill_id.clone(),
            target_root_id: planned.cell.target_root_id.clone(),
            outcome,
            diagnostic,
        }
    }

    fn build_journal(&self, batch: &PlannedBatch, library_root: &Path) -> EnableJournal {
        let cells = batch
            .cells
            .iter()
            .enumerate()
            .map(|(index, planned)| {
                let runnable = planned.cell.eligibility == CellEligibility::Ready
                    || (planned.cell.eligibility == CellEligibility::Conflict
                        && effective_resolution(planned) != CellResolution::Skip);
                let phase = if runnable {
                    ActivationReplacePhase::Applying
                } else {
                    ActivationReplacePhase::Committed
                };
                EnableJournalCell {
                    cell_index: index as u32,
                    action: effective_action(planned),
                    skill_id: planned.cell.skill_id.0.clone(),
                    target_root_id: planned.cell.target_root_id.clone(),
                    directory_identity_key: planned.cell.directory_identity_key.clone(),
                    entry_path: planned.cell.entry_path.clone(),
                    target_path: planned.cell.final_entity_path.clone(),
                    target_parent: planned.frozen_target.clone(),
                    after_desired: grid_after_desired(planned),
                    before_desired: planned.before_desired,
                    backup_path: backup_path_for(batch, index, library_root),
                    occupant: journal_occupant(planned),
                    phase,
                }
            })
            .collect();
        EnableJournal {
            version: JOURNAL_VERSION,
            operation_id: batch.operation_id.clone(),
            phase: ActivationReplacePhase::Applying,
            cells,
        }
    }

    /// Re-verify the full frozen evidence immediately before the first
    /// write (spec §4.9): any generation, identity, health or occupancy
    /// change → `PlanStale`.
    fn verify_plan(&self, batch: &PlannedBatch) -> Result<(), EnableError> {
        let agent_snapshot = self.agent_store.agent_configuration_snapshot()?;
        if agent_snapshot.snapshot_version != batch.agent_generation {
            return Err(EnableError::PlanStale);
        }
        if self.store.catalog_generation()? != batch.catalog_generation {
            return Err(EnableError::PlanStale);
        }
        if batch.scope == "project" {
            let canonical_root = batch
                .canonical_project_root
                .as_ref()
                .ok_or(EnableError::PlanStale)?;
            let expected_root = batch
                .project_root_identity
                .as_ref()
                .ok_or(EnableError::PlanStale)?;
            let actual_root = self
                .filesystem
                .directory_fingerprint(canonical_root)
                .map_err(|_| EnableError::PlanStale)?;
            if &actual_root != expected_root {
                return Err(EnableError::PlanStale);
            }
            for planned in &batch.cells {
                let cell = &planned.cell;
                for frozen_target in &planned.frozen_project_targets {
                    let current = self
                        .filesystem
                        .resolve_project_target(
                            canonical_root,
                            &frozen_target.configured_relative_path,
                        )
                        .map_err(|_| EnableError::PlanStale)?;
                    if current != frozen_target.resolution
                        || current.resolved_container != cell.target_path
                    {
                        return Err(EnableError::PlanStale);
                    }
                }
                if let Some(expected_target) = &planned.frozen_target {
                    let actual_target = self
                        .filesystem
                        .directory_fingerprint(&cell.target_path)
                        .map_err(|_| EnableError::PlanStale)?;
                    if &actual_target != expected_target {
                        return Err(EnableError::PlanStale);
                    }
                }
                let skill = self
                    .catalog
                    .inspect(&cell.skill_id)?
                    .ok_or(EnableError::PlanStale)?;
                if skill.summary.health != planned.frozen_health
                    || skill.summary.source_kind != planned.frozen_source_kind
                    || skill.final_entity_path != planned.frozen_final_entity.to_string_lossy()
                {
                    return Err(EnableError::PlanStale);
                }
                if matches!(
                    planned.cell.eligibility,
                    CellEligibility::Ready | CellEligibility::Conflict
                ) && let Some(service) = &self.source_update
                    && service.ensure_new_enable_allowed(&cell.skill_id).is_err()
                {
                    return Err(EnableError::PlanStale);
                }
                match (
                    &planned.frozen_entry,
                    self.filesystem.activation_snapshot(&cell.entry_path)?,
                ) {
                    (None, ActivationEntrySnapshot::Missing) => {}
                    (Some(expected), _) => {
                        let observed = self.filesystem.occupant_snapshot(&cell.entry_path)?;
                        if &observed != expected {
                            return Err(EnableError::PlanStale);
                        }
                    }
                    (None, _) => return Err(EnableError::PlanStale),
                }
            }
            return Ok(());
        }
        for planned in &batch.cells {
            let cell = &planned.cell;
            let skill = self
                .catalog
                .inspect(&cell.skill_id)?
                .ok_or(EnableError::PlanStale)?;
            if skill.summary.health != planned.frozen_health
                || skill.summary.source_kind != planned.frozen_source_kind
                || skill.final_entity_path != planned.frozen_final_entity.to_string_lossy()
            {
                return Err(EnableError::PlanStale);
            }
            let root = agent_snapshot
                .roots
                .iter()
                .find(|root| root.root_id == cell.target_root_id)
                .ok_or(EnableError::PlanStale)?;
            if !agent_snapshot
                .configurations
                .iter()
                .any(|configuration| references_target(configuration, &root))
            {
                return Err(EnableError::PlanStale);
            }
            match self.agent_fs.probe_root(&root.configured_path)? {
                AgentRootProbe::Present { canonical_path }
                    if planned.frozen_availability == TargetGroupAvailability::Available
                        && planned.frozen_target.as_ref().is_some_and(|target| {
                            target.canonical_path == canonical_path
                                && target.canonical_path == cell.target_path
                        })
                        && self
                            .filesystem
                            .directory_fingerprint(&root.configured_path)
                            .is_ok_and(|actual| {
                                planned.frozen_target.as_ref() == Some(&actual)
                            }) => {}
                AgentRootProbe::Absent
                    if planned.frozen_availability == TargetGroupAvailability::Absent => {}
                AgentRootProbe::Unavailable { .. }
                    if planned.frozen_availability == TargetGroupAvailability::Unavailable => {}
                _ => return Err(EnableError::PlanStale),
            }
            if matches!(
                planned.cell.action,
                EnableAction::Enable | EnableAction::Repair | EnableAction::Switch
            ) && matches!(
                planned.cell.eligibility,
                CellEligibility::Ready | CellEligibility::Conflict
            ) {
                if let Some(service) = &self.source_update {
                    if service.ensure_new_enable_allowed(&cell.skill_id).is_err() {
                        return Err(EnableError::PlanStale);
                    }
                }
            }
            // Entry occupancy CAS: the entry must still hold the exact
            // planned condition.
            match (
                &planned.frozen_entry,
                self.filesystem.activation_snapshot(&cell.entry_path)?,
            ) {
                (None, ActivationEntrySnapshot::Missing) => {}
                (Some(expected), _) => {
                    let observed = self.filesystem.occupant_snapshot(&cell.entry_path)?;
                    if &observed != expected {
                        return Err(EnableError::PlanStale);
                    }
                }
                (None, _) => return Err(EnableError::PlanStale),
            }
            // The desired state must still equal the frozen before state.
            let rows = self.store.activation_cells_for_skill(&cell.skill_id)?;
            let current_desired = rows
                .iter()
                .find(|row| row.target_root_id == cell.target_root_id)
                .map(|row| row.desired_enabled)
                .unwrap_or(false);
            if current_desired != planned.before_desired {
                return Err(EnableError::PlanStale);
            }
        }
        Ok(())
    }

    /// Commit one cell. The journal cell is already `Applying` (persisted),
    /// so a crash at any point is recoverable by `recover_enable_journals`.
    fn apply_cell(
        &self,
        planned: &PlannedCell,
        journal: &mut EnableJournal,
        index: usize,
        library_root: &Path,
        scope: &str,
        project_root_identity: Option<&DirectoryFingerprint>,
    ) -> Result<u64, EnableError> {
        let cell = &planned.cell;
        if scope == "project" {
            match effective_action(planned) {
                EnableCellAction::Enable | EnableCellAction::Repair => {
                    let project_root_identity =
                        project_root_identity.ok_or(EnableError::PlanStale)?;
                    let target_identity = self.filesystem.create_project_activation(
                        project_root_identity,
                        &cell.target_path,
                        &cell.create_steps,
                        &cell.entry_path,
                        &cell.final_entity_path,
                        planned.frozen_target.as_ref(),
                    )?;
                    journal.cells[index].target_parent = Some(target_identity);
                    journal.cells[index].phase = ActivationReplacePhase::Committed;
                    journal.cells[index].after_desired = true;
                    self.filesystem
                        .write_enable_journal(library_root, journal)?;
                    return Ok(self.store.catalog_generation()?);
                }
                EnableCellAction::Replace => {
                    let occupant = journal.cells[index]
                        .occupant
                        .clone()
                        .ok_or(EnableError::PlanStale)?;
                    let backup_path = journal.cells[index]
                        .backup_path
                        .clone()
                        .ok_or(EnableError::PlanStale)?;
                    let observed = self.filesystem.occupant_snapshot(&cell.entry_path)?;
                    if observed != occupant {
                        return Err(EnableError::CellConflict(
                            "the occupant changed before the replace".into(),
                        ));
                    }
                    let target_parent = planned
                        .frozen_target
                        .as_ref()
                        .ok_or(EnableError::PlanStale)?;
                    self.filesystem.move_occupant_to_backup_nofollow(
                        &cell.entry_path,
                        &backup_path,
                        library_root,
                        &occupant,
                        target_parent,
                    )?;
                    let project_root_identity =
                        project_root_identity.ok_or(EnableError::PlanStale)?;
                    let target_identity = self.filesystem.create_project_activation(
                        project_root_identity,
                        &cell.target_path,
                        &cell.create_steps,
                        &cell.entry_path,
                        &cell.final_entity_path,
                        Some(target_parent),
                    )?;
                    journal.cells[index].target_parent = Some(target_identity);
                    journal.cells[index].phase = ActivationReplacePhase::Committed;
                    journal.cells[index].after_desired = true;
                    self.filesystem
                        .write_enable_journal(library_root, journal)?;
                    return Ok(self.store.catalog_generation()?);
                }
                _ => {
                    return Err(EnableError::Validation(
                        "unsupported action for project enable".into(),
                    ));
                }
            }
        }
        let write_owner = ActivationCellWrite {
            skill_id: cell.skill_id.clone(),
            target_root_id: cell.target_root_id.clone(),
            directory_identity_key: cell.directory_identity_key.clone(),
            desired_enabled: true,
            expected_entry_path: cell.entry_path.clone(),
            expected_target_path: cell.final_entity_path.clone(),
        };
        let write_owner_disabled = ActivationCellWrite {
            desired_enabled: false,
            ..write_owner.clone()
        };

        match effective_action(planned) {
            EnableCellAction::Enable | EnableCellAction::Repair => {
                // Plain create (no occupant): the entry must be missing.
                if !matches!(
                    self.filesystem.activation_snapshot(&cell.entry_path)?,
                    ActivationEntrySnapshot::Missing
                ) {
                    return Err(EnableError::CellConflict(
                        "the entry is no longer missing".into(),
                    ));
                }
                let target_parent = planned
                    .frozen_target
                    .as_ref()
                    .ok_or(EnableError::PlanStale)?;
                self.filesystem.create_activation_nofollow(
                    &cell.final_entity_path,
                    &cell.entry_path,
                    target_parent,
                )?;
                journal.cells[index].phase = ActivationReplacePhase::Committed;
                if cell.action == EnableAction::Enable {
                    journal.cells[index].after_desired = true;
                    let version = match self.store.write_activation_cells(&[write_owner]) {
                        Ok(version) => version,
                        Err(error) => {
                            if let Err(compensation) = self.compensate_create(
                                &cell.entry_path,
                                &cell.final_entity_path,
                                target_parent,
                            ) {
                                journal.cells[index].phase = ActivationReplacePhase::Applying;
                                self.filesystem
                                    .write_enable_journal(library_root, journal)?;
                                return Err(self.block_for_recovery(
                                    "remove the uncommitted Enable entry",
                                    format!("{error}; compensation also failed: {compensation}"),
                                ));
                            }
                            journal.cells[index].phase = ActivationReplacePhase::Committed;
                            self.filesystem
                                .write_enable_journal(library_root, journal)?;
                            return Err(EnableError::from(error));
                        }
                    };
                    self.filesystem
                        .write_enable_journal(library_root, journal)?;
                    Ok(version)
                } else {
                    // Repair: the catalog already desired; the entry is the
                    // commit point.
                    journal.cells[index].after_desired = true;
                    self.filesystem
                        .write_enable_journal(library_root, journal)?;
                    Ok(self.store.catalog_generation()?)
                }
            }
            EnableCellAction::Disable => {
                let entry_state = self.filesystem.activation_snapshot(&cell.entry_path)?;
                let removed = match entry_state {
                    ActivationEntrySnapshot::Symlink { target }
                        if target == cell.final_entity_path =>
                    {
                        let target_parent = planned
                            .frozen_target
                            .as_ref()
                            .ok_or(EnableError::PlanStale)?;
                        self.filesystem.remove_activation_nofollow(
                            &cell.final_entity_path,
                            &cell.entry_path,
                            target_parent,
                        )?;
                        true
                    }
                    ActivationEntrySnapshot::Missing => false,
                    _ => {
                        // External content: only the catalog write applies;
                        // the entry is never touched.
                        false
                    }
                };
                journal.cells[index].phase = ActivationReplacePhase::Committed;
                journal.cells[index].after_desired = false;
                let version = match self.store.write_activation_cells(&[write_owner_disabled]) {
                    Ok(version) => version,
                    Err(error) => {
                        if removed {
                            if let Err(compensation) = self.filesystem.create_activation_nofollow(
                                &cell.final_entity_path,
                                &cell.entry_path,
                                planned
                                    .frozen_target
                                    .as_ref()
                                    .ok_or(EnableError::PlanStale)?,
                            ) {
                                journal.cells[index].phase = ActivationReplacePhase::Applying;
                                self.filesystem
                                    .write_enable_journal(library_root, journal)?;
                                return Err(self.block_for_recovery(
                                    "restore the removed Disable entry",
                                    compensation,
                                ));
                            }
                            journal.cells[index].phase = ActivationReplacePhase::Committed;
                            self.filesystem
                                .write_enable_journal(library_root, journal)?;
                        }
                        return Err(EnableError::from(error));
                    }
                };
                self.filesystem
                    .write_enable_journal(library_root, journal)?;
                Ok(version)
            }
            EnableCellAction::Switch | EnableCellAction::Replace => {
                let occupant = journal.cells[index]
                    .occupant
                    .clone()
                    .ok_or(EnableError::PlanStale)?;
                let backup_path = journal.cells[index]
                    .backup_path
                    .clone()
                    .ok_or(EnableError::PlanStale)?;
                // The entry must still match the frozen occupier.
                let observed = self.filesystem.occupant_snapshot(&cell.entry_path)?;
                if observed != occupant {
                    return Err(EnableError::CellConflict(
                        "the occupant changed before the replace".into(),
                    ));
                }
                let target_parent = planned
                    .frozen_target
                    .as_ref()
                    .ok_or(EnableError::PlanStale)?;
                self.filesystem.move_occupant_to_backup_nofollow(
                    &cell.entry_path,
                    &backup_path,
                    library_root,
                    &occupant,
                    target_parent,
                )?;
                self.filesystem.create_activation_nofollow(
                    &cell.final_entity_path,
                    &cell.entry_path,
                    target_parent,
                )?;
                journal.cells[index].phase = ActivationReplacePhase::Committed;
                journal.cells[index].after_desired = true;
                // Deactivate the old owner FIRST: the partial enabled-entry
                // index forbids two enabled rows with the same
                // (Target, Directory Identity) at any instant.
                let mut writes = Vec::new();
                if let Occupier::Managed {
                    skill_id: old_skill,
                    ..
                } = &cell.occupancy
                {
                    let old_rows = self.store.activation_cells_for_skill(old_skill)?;
                    if let Some(old_row) = old_rows
                        .iter()
                        .find(|row| row.target_root_id == cell.target_root_id)
                    {
                        writes.push(ActivationCellWrite {
                            skill_id: old_skill.clone(),
                            target_root_id: cell.target_root_id.clone(),
                            directory_identity_key: old_row.directory_identity_key.clone(),
                            desired_enabled: false,
                            expected_entry_path: old_row.expected_entry_path.clone(),
                            expected_target_path: old_row.expected_target_path.clone(),
                        });
                    }
                }
                writes.push(write_owner);
                let version = match self.store.write_activation_cells(&writes) {
                    Ok(version) => version,
                    Err(error) => {
                        // Restore the occupant and remove the new link.
                        match self.filesystem.activation_snapshot(&cell.entry_path)? {
                            ActivationEntrySnapshot::Symlink { target }
                                if target == cell.final_entity_path =>
                            {
                                self.filesystem.remove_activation_nofollow(
                                    &cell.final_entity_path,
                                    &cell.entry_path,
                                    target_parent,
                                )?;
                            }
                            _ => {}
                        }
                        let restore = self.filesystem.restore_occupant_from_backup_nofollow(
                            &backup_path,
                            &cell.entry_path,
                            library_root,
                            &occupant,
                            target_parent,
                        );
                        match restore {
                            Ok(()) => {
                                journal.cells[index].phase = ActivationReplacePhase::Committed;
                                self.filesystem
                                    .write_enable_journal(library_root, journal)?;
                                return Err(EnableError::from(error));
                            }
                            Err(compensation) => {
                                journal.cells[index].phase = ActivationReplacePhase::Applying;
                                self.filesystem
                                    .write_enable_journal(library_root, journal)?;
                                return Err(self.block_for_recovery(
                                    "restore the replaced occupant",
                                    format!("{error}; restore also failed: {compensation}"),
                                ));
                            }
                        }
                    }
                };
                self.filesystem
                    .write_enable_journal(library_root, journal)?;
                Ok(version)
            }
        }
    }

    /// Compensate a failed plain create: remove the entry we just created
    /// (only when it still is our link).
    fn compensate_create(
        &self,
        entry_path: &Path,
        final_entity: &Path,
        expected_parent: &DirectoryFingerprint,
    ) -> Result<(), EnableError> {
        match self.filesystem.activation_snapshot(entry_path)? {
            ActivationEntrySnapshot::Symlink { target } if target == final_entity => {
                self.filesystem.remove_activation_nofollow(
                    final_entity,
                    entry_path,
                    expected_parent,
                )?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Undo the whole operation while its result window is open: each
    /// succeeded cell is CAS-verified first; cells changed externally are
    /// rejected on their own and the others continue (spec §4.9).
    pub fn undo(&self, operation_id: &str) -> Result<EnableUndoResult, EnableError> {
        self.ensure_writes_ready()?;
        let batch = self
            .applied
            .lock()
            .map_err(|_| EnableError::Internal("Enable applied lock poisoned".into()))?
            .remove(operation_id)
            .ok_or(EnableError::PlanNotFound)?;
        let library_root = self.library_root_for_context(&batch.write_context)?;
        let _write_guard = self.acquire_write_guard(&batch.write_context)?;
        let mut journal = self.build_journal(&batch, &library_root);
        let succeeded = batch
            .cells
            .iter()
            .enumerate()
            .filter(|(_, planned)| {
                planned.cell.eligibility == CellEligibility::Ready
                    || (planned.cell.eligibility == CellEligibility::Conflict
                        && effective_resolution(planned) != CellResolution::Skip)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut results = Vec::with_capacity(succeeded.len());
        let mut snapshot_version = self.store.catalog_generation()?;
        for index in succeeded.into_iter().rev() {
            let planned = &batch.cells[index];
            match self.verify_undo_cas(
                planned,
                &journal.cells[index],
                batch.scope,
                batch.project_root_identity.as_ref(),
            ) {
                Ok(()) => {}
                Err(EnableError::Validation(message)) => {
                    results.push(EnableUndoCellResult {
                        cell_key: planned.cell.cell_key.clone(),
                        undone: false,
                        diagnostic: Some(message),
                    });
                    continue;
                }
                Err(error) => return Err(error),
            }
            match self.undo_cell(
                planned,
                &mut journal,
                index,
                &library_root,
                batch.scope,
                batch.project_root_identity.as_ref(),
            ) {
                Ok(version) => {
                    snapshot_version = version;
                    results.push(EnableUndoCellResult {
                        cell_key: planned.cell.cell_key.clone(),
                        undone: true,
                        diagnostic: None,
                    });
                }
                Err(error) => {
                    return Err(self.block_for_recovery(
                        &format!("undo Enable cell '{}'", planned.cell.cell_key),
                        error,
                    ));
                }
            }
        }
        if results.iter().any(|result| result.undone) {
            if let Err(error) = self
                .filesystem
                .finish_enable_journal(&library_root, operation_id)
            {
                return Err(self.block_for_recovery("archive completed Enable Undo", error));
            }
        } else if let Err(error) = self
            .filesystem
            .finish_enable_journal(&library_root, operation_id)
        {
            return Err(self.block_for_recovery("archive empty Enable Undo", error));
        }
        Ok(EnableUndoResult {
            operation_id: operation_id.to_owned(),
            cells: results,
            snapshot_version,
        })
    }

    /// Per-cell CAS: the entry must still be our created link (or, for a
    /// Disable undo, still absent), the backup must still match, and the
    /// catalog must still hold the committed state. An externally changed
    /// cell is a normal per-cell rejection.
    fn verify_undo_cas(
        &self,
        planned: &PlannedCell,
        journal_cell: &EnableJournalCell,
        scope: &str,
        project_root_identity: Option<&DirectoryFingerprint>,
    ) -> Result<(), EnableError> {
        let cell = &planned.cell;
        if scope == "project" {
            let project_root_identity = project_root_identity.ok_or(EnableError::PlanStale)?;
            let actual_root = self
                .filesystem
                .directory_fingerprint(&project_root_identity.canonical_path)
                .map_err(|_| EnableError::PlanStale)?;
            if &actual_root != project_root_identity {
                return Err(EnableError::Validation(
                    "the project root changed since the operation; skipping".into(),
                ));
            }
            for frozen_target in &planned.frozen_project_targets {
                let current = self
                    .filesystem
                    .resolve_project_target(
                        &project_root_identity.canonical_path,
                        &frozen_target.configured_relative_path,
                    )
                    .map_err(|_| {
                        EnableError::Validation(
                            "the project target changed since the operation; skipping".into(),
                        )
                    })?;
                let resolution_matches = if frozen_target.resolution.create_steps.is_empty() {
                    current == frozen_target.resolution
                } else {
                    current.fault.is_none()
                        && current.resolved_container == cell.target_path
                        && current.create_steps.is_empty()
                };
                if !resolution_matches {
                    return Err(EnableError::Validation(
                        "the project target changed since the operation; skipping".into(),
                    ));
                }
            }
            let actual_target = self
                .filesystem
                .directory_fingerprint(&cell.target_path)
                .map_err(|_| {
                    EnableError::Validation("the project target is unavailable; skipping".into())
                })?;
            if let Some(expected_target) = &planned.frozen_target
                && &actual_target != expected_target
            {
                return Err(EnableError::Validation(
                    "the project target changed since the operation; skipping".into(),
                ));
            }
            let entry_state = self.filesystem.activation_snapshot(&cell.entry_path)?;
            if !matches!(
                entry_state,
                ActivationEntrySnapshot::Symlink { ref target }
                    if target == &cell.final_entity_path
            ) {
                return Err(EnableError::Validation(
                    "the entry was changed since the operation; skipping".into(),
                ));
            }
            if let (Some(backup_path), Some(occupant)) = (
                journal_cell.backup_path.clone(),
                journal_cell.occupant.clone(),
            ) {
                match self.filesystem.occupant_snapshot(&backup_path) {
                    Ok(backup) if backup.kind == occupant.kind => {}
                    Ok(_) => {
                        return Err(EnableError::Validation(
                            "the operation backup was changed; skipping".into(),
                        ));
                    }
                    Err(FileSystemError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        return Err(EnableError::Validation(
                            "the operation backup is gone; skipping".into(),
                        ));
                    }
                    Err(error) => return Err(EnableError::from(error)),
                }
            }
            return Ok(());
        }
        let expected_target = planned
            .frozen_target
            .as_ref()
            .ok_or(EnableError::PlanStale)?;
        let actual_target = self
            .filesystem
            .directory_fingerprint(&cell.target_path)
            .map_err(|_| {
                EnableError::Validation(
                    "the Activation Target changed since the operation; skipping".into(),
                )
            })?;
        if &actual_target != expected_target {
            return Err(EnableError::Validation(
                "the Activation Target changed since the operation; skipping".into(),
            ));
        }
        let entry_state = self.filesystem.activation_snapshot(&cell.entry_path)?;
        match cell.action {
            EnableAction::Enable | EnableAction::Repair | EnableAction::Switch => {
                if !matches!(
                    entry_state,
                    ActivationEntrySnapshot::Symlink { ref target }
                        if target == &cell.final_entity_path
                ) {
                    return Err(EnableError::Validation(
                        "the entry was changed since the operation; skipping".into(),
                    ));
                }
            }
            EnableAction::Disable => {
                if !matches!(entry_state, ActivationEntrySnapshot::Missing) {
                    return Err(EnableError::Validation(
                        "the entry is no longer absent; skipping".into(),
                    ));
                }
            }
        }
        if let (Some(backup_path), Some(occupant)) = (
            journal_cell.backup_path.clone(),
            journal_cell.occupant.clone(),
        ) {
            match self.filesystem.occupant_snapshot(&backup_path) {
                Ok(backup) if backup.kind == occupant.kind => {}
                Ok(_) => {
                    return Err(EnableError::Validation(
                        "the operation backup was changed; skipping".into(),
                    ));
                }
                Err(FileSystemError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(EnableError::Validation(
                        "the operation backup is gone; skipping".into(),
                    ));
                }
                Err(error) => return Err(EnableError::from(error)),
            }
        }
        let rows = self.store.activation_cells_for_skill(&cell.skill_id)?;
        let desired = rows
            .iter()
            .find(|row| row.target_root_id == cell.target_root_id)
            .map(|row| row.desired_enabled)
            .unwrap_or(false);
        if desired != grid_after_desired(planned) {
            return Err(EnableError::Validation(
                "the catalog state changed since the operation; skipping".into(),
            ));
        }
        Ok(())
    }

    /// Restore one cell's before state. Journal phase is `Undoing` before
    /// the first mutation so an interrupted Undo completes at startup.
    fn undo_cell(
        &self,
        planned: &PlannedCell,
        journal: &mut EnableJournal,
        index: usize,
        library_root: &Path,
        scope: &str,
        project_root_identity: Option<&DirectoryFingerprint>,
    ) -> Result<u64, EnableError> {
        let cell = &planned.cell;
        if scope == "project" {
            let _project_root_identity = project_root_identity.ok_or(EnableError::PlanStale)?;
            let target_parent = self.filesystem.directory_fingerprint(&cell.target_path)?;
            journal.cells[index].phase = ActivationReplacePhase::Undoing;
            self.filesystem
                .write_enable_journal(library_root, journal)?;
            if matches!(
                self.filesystem.activation_snapshot(&cell.entry_path)?,
                ActivationEntrySnapshot::Symlink { ref target }
                    if target == &cell.final_entity_path
            ) {
                self.filesystem.remove_activation_nofollow(
                    &cell.final_entity_path,
                    &cell.entry_path,
                    &target_parent,
                )?;
            }
            if let (Some(backup_path), Some(occupant)) = (
                journal.cells[index].backup_path.clone(),
                journal.cells[index].occupant.clone(),
            ) {
                self.filesystem.restore_occupant_from_backup_nofollow(
                    &backup_path,
                    &cell.entry_path,
                    library_root,
                    &occupant,
                    &target_parent,
                )?;
            }
            return Ok(self.store.catalog_generation()?);
        }
        journal.cells[index].phase = ActivationReplacePhase::Undoing;
        self.filesystem
            .write_enable_journal(library_root, journal)?;
        let write_owner = ActivationCellWrite {
            skill_id: cell.skill_id.clone(),
            target_root_id: cell.target_root_id.clone(),
            directory_identity_key: cell.directory_identity_key.clone(),
            desired_enabled: planned.before_desired,
            expected_entry_path: cell.entry_path.clone(),
            expected_target_path: cell.final_entity_path.clone(),
        };
        let target_parent = planned
            .frozen_target
            .as_ref()
            .ok_or(EnableError::PlanStale)?;
        let effective = effective_action(planned);
        let restores_occupant = matches!(
            effective,
            EnableCellAction::Switch | EnableCellAction::Replace
        );
        match effective {
            EnableCellAction::Enable | EnableCellAction::Repair => {
                if matches!(
                    self.filesystem.activation_snapshot(&cell.entry_path)?,
                    ActivationEntrySnapshot::Symlink { ref target }
                        if target == &cell.final_entity_path
                ) {
                    self.filesystem.remove_activation_nofollow(
                        &cell.final_entity_path,
                        &cell.entry_path,
                        target_parent,
                    )?;
                }
                Ok(self.store.write_activation_cells(&[write_owner])?)
            }
            EnableCellAction::Disable => {
                self.filesystem.create_activation_nofollow(
                    &cell.final_entity_path,
                    &cell.entry_path,
                    target_parent,
                )?;
                self.store.write_activation_cells(&[write_owner])?;
                Ok(self.store.catalog_generation()?)
            }
            EnableCellAction::Switch | EnableCellAction::Replace => {
                if matches!(
                    self.filesystem.activation_snapshot(&cell.entry_path)?,
                    ActivationEntrySnapshot::Symlink { ref target }
                        if target == &cell.final_entity_path
                ) {
                    self.filesystem.remove_activation_nofollow(
                        &cell.final_entity_path,
                        &cell.entry_path,
                        target_parent,
                    )?;
                }
                if let (Some(backup_path), Some(occupant)) = (
                    journal.cells[index].backup_path.clone(),
                    journal.cells[index].occupant.clone(),
                ) {
                    self.filesystem.restore_occupant_from_backup_nofollow(
                        &backup_path,
                        &cell.entry_path,
                        library_root,
                        &occupant,
                        target_parent,
                    )?;
                }
                let mut writes = vec![write_owner];
                if restores_occupant
                    && let Occupier::Managed {
                        skill_id: old_skill,
                        ..
                    } = &cell.occupancy
                {
                    let old_rows = self.store.activation_cells_for_skill(old_skill)?;
                    if let Some(old_row) = old_rows
                        .iter()
                        .find(|row| row.target_root_id == cell.target_root_id)
                    {
                        writes.push(ActivationCellWrite {
                            skill_id: old_skill.clone(),
                            target_root_id: cell.target_root_id.clone(),
                            directory_identity_key: old_row.directory_identity_key.clone(),
                            desired_enabled: true,
                            expected_entry_path: old_row.expected_entry_path.clone(),
                            expected_target_path: old_row.expected_target_path.clone(),
                        });
                    }
                }
                self.store.write_activation_cells(&writes)?;
                Ok(self.store.catalog_generation()?)
            }
        }
    }

    /// Finalize: discard every identity/CAS-verified committed backup and
    /// close the Undo window. Backups that no longer match are refused
    /// (RecoveryRequired), never force-deleted.
    pub fn finalize(&self, operation_id: &str) -> Result<(), EnableError> {
        self.ensure_writes_ready()?;
        let batch = self
            .applied
            .lock()
            .map_err(|_| EnableError::Internal("Enable applied lock poisoned".into()))?
            .remove(operation_id)
            .ok_or(EnableError::PlanNotFound)?;
        let library_root = self.library_root_for_context(&batch.write_context)?;
        let _write_guard = self.acquire_write_guard(&batch.write_context)?;
        let journal = self.build_journal(&batch, &library_root);
        for cell in &journal.cells {
            if let (Some(backup_path), Some(occupant)) = (&cell.backup_path, &cell.occupant) {
                self.filesystem
                    .discard_replace_backup(backup_path, &library_root, occupant)?;
            }
        }
        if let Err(error) = self
            .filesystem
            .finish_enable_journal(&library_root, operation_id)
        {
            return Err(self.block_for_recovery("finalize Enable operation", error));
        }
        Ok(())
    }

    pub fn list_recent_project_folders(&self) -> Result<Vec<RecentProjectFolder>, EnableError> {
        Ok(self.agent_store.list_recent_project_folders()?)
    }

    pub fn clear_recent_project_folders(&self) -> Result<(), EnableError> {
        let context = self.capture_write_context()?;
        let _write_guard = self.acquire_write_guard(&context)?;
        Ok(self.agent_store.clear_recent_project_folders()?)
    }
}

/// The journal action this cell actually executes (resolutions rewrite
/// Enable into Switch/Replace).
fn effective_action(planned: &PlannedCell) -> EnableCellAction {
    let cell = &planned.cell;
    if cell.eligibility == CellEligibility::Conflict {
        return match effective_resolution(planned) {
            CellResolution::Switch => EnableCellAction::Switch,
            CellResolution::Replace => EnableCellAction::Replace,
            CellResolution::Adopt | CellResolution::Skip => EnableCellAction::Enable,
        };
    }
    match cell.action {
        EnableAction::Enable => EnableCellAction::Enable,
        EnableAction::Disable => EnableCellAction::Disable,
        EnableAction::Repair => EnableCellAction::Repair,
        EnableAction::Switch => EnableCellAction::Switch,
    }
}

/// The user resolution that actually drives apply (exact-direct links are
/// always Replace; managed occupiers are always Switch per ADR-0019).
fn effective_resolution(planned: &PlannedCell) -> CellResolution {
    let cell = &planned.cell;
    match &cell.occupancy {
        Occupier::Managed { .. } => CellResolution::Switch,
        Occupier::Empty => cell.resolution,
        Occupier::Untracked { .. } => {
            if cell.occ_exact_direct {
                CellResolution::Replace
            } else if cell.action == EnableAction::Enable {
                match cell.resolution {
                    CellResolution::Replace
                    | CellResolution::Adopt
                    | CellResolution::Skip
                    | CellResolution::Switch => cell.resolution,
                }
            } else {
                cell.resolution
            }
        }
    }
}

/// The catalog desired state after the cell's commit point.
fn grid_after_desired(planned: &PlannedCell) -> bool {
    if planned.cell.eligibility == CellEligibility::Conflict {
        return matches!(
            effective_action(planned),
            EnableCellAction::Enable | EnableCellAction::Switch | EnableCellAction::Replace
        );
    }
    match planned.cell.action {
        EnableAction::Enable | EnableAction::Switch => true,
        EnableAction::Disable => false,
        // Repair keeps the existing desired state.
        EnableAction::Repair => true,
    }
}

fn journal_occupant(planned: &PlannedCell) -> Option<OccupantSnapshot> {
    match effective_action(planned) {
        EnableCellAction::Switch | EnableCellAction::Replace => planned.frozen_entry.clone(),
        _ => None,
    }
}

fn backup_path_for(batch: &PlannedBatch, index: usize, library_root: &Path) -> Option<PathBuf> {
    if !matches!(
        effective_action(&batch.cells[index]),
        EnableCellAction::Switch | EnableCellAction::Replace
    ) || batch.cells[index].frozen_entry.is_none()
    {
        return None;
    }
    Some(
        library_root
            .join("operations")
            .join(&batch.operation_id)
            .join("backup")
            .join(format!("cell-{index}")),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateCode {
    Ok,
    Mismatch,
    Tombstoned,
}

fn map_gate_code(error: Result<(), SourceUpdateError>) -> GateCode {
    match error {
        Ok(()) => GateCode::Ok,
        Err(SourceUpdateError::SourceSnapshotMismatch) => GateCode::Mismatch,
        Err(_) => GateCode::Tombstoned,
    }
}

fn references_target(
    configuration: &StoredAgentConfiguration,
    root: &StoredGlobalSkillRoot,
) -> bool {
    configuration.memberships.iter().any(|membership| {
        membership.root_id == root.root_id
            && membership.role == crate::core::agent_configuration::AgentRootRole::ActivationTarget
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_cell(
    action: EnableAction,
    frozen_entry: &Option<OccupantSnapshot>,
    occupier: &Occupier,
    exact_direct: bool,
    entry_is_ours: bool,
    before_desired: bool,
    health: Health,
    source_gate: GateCode,
    availability: TargetGroupAvailability,
    current_row: Option<&ActivationCellRow>,
    cell_key: &str,
    resolutions: &[(String, CellResolution)],
    is_batch: bool,
) -> (
    CellEligibility,
    Option<CellBlockedReason>,
    CellResolution,
    Option<String>,
) {
    let resolution_of = |key: &str| {
        resolutions
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, r)| *r)
            .unwrap_or(CellResolution::Skip)
    };
    let occupied = frozen_entry.is_some();
    let is_ready_entity = health == Health::Healthy && source_gate == GateCode::Ok;

    match action {
        EnableAction::Disable => {
            if !before_desired {
                return (CellEligibility::NoOp, None, CellResolution::Skip, None);
            }
            match frozen_entry {
                None => (CellEligibility::Ready, None, CellResolution::Skip, None),
                Some(snapshot) => match &snapshot.kind {
                    // Our own link (whatever Managed row it belongs to —
                    // ours): normal Disable.
                    OccupantKind::Symlink { .. } if entry_is_ours => {
                        (CellEligibility::Ready, None, CellResolution::Skip, None)
                    }
                    _ => (
                        CellEligibility::Blocked,
                        Some(CellBlockedReason::EntryOccupied),
                        CellResolution::Skip,
                        None,
                    ),
                },
            }
        }
        EnableAction::Repair => {
            if !before_desired {
                return (CellEligibility::NoOp, None, CellResolution::Skip, None);
            }
            match frozen_entry {
                None if is_ready_entity => {
                    (CellEligibility::Ready, None, CellResolution::Skip, None)
                }
                None => blocked_cell(health, source_gate, availability),
                Some(snapshot) => match &snapshot.kind {
                    OccupantKind::Symlink { .. } => {
                        // Entry already points somewhere: Repair only covers
                        // missing entries (ADR-0019 Repair definition).
                        if matches!(occupier, Occupier::Managed { .. }) {
                            (CellEligibility::Conflict, None, CellResolution::Skip, None)
                        } else {
                            (CellEligibility::NoOp, None, CellResolution::Skip, None)
                        }
                    }
                    _ => (
                        CellEligibility::Blocked,
                        Some(CellBlockedReason::EntryOccupied),
                        CellResolution::Skip,
                        None,
                    ),
                },
            }
        }
        EnableAction::Switch | EnableAction::Enable => {
            if before_desired {
                return match frozen_entry {
                    // Our own link: already enabled.
                    Some(snapshot)
                        if matches!(&snapshot.kind, OccupantKind::Symlink { .. })
                            && !matches!(occupier, Occupier::Managed { .. }) =>
                    {
                        (CellEligibility::NoOp, None, CellResolution::Skip, None)
                    }
                    None => (
                        CellEligibility::Blocked,
                        Some(CellBlockedReason::EntityBroken),
                        CellResolution::Skip,
                        None,
                    ),
                    _ => (CellEligibility::NoOp, None, CellResolution::Skip, None),
                };
            }
            if !occupied {
                if is_ready_entity && availability == TargetGroupAvailability::Available {
                    return (CellEligibility::Ready, None, CellResolution::Skip, None);
                }
                return blocked_cell(health, source_gate, availability);
            }
            let blocked = match_source_gate(source_gate, availability);
            if let Some(reason) = blocked {
                return (
                    CellEligibility::Blocked,
                    Some(reason),
                    CellResolution::Skip,
                    None,
                );
            }
            let mut resolution = match occupier {
                // Managed ownership: the only resolution is a Target-local Switch.
                Occupier::Managed { .. } => {
                    // Managed ownership: the only resolution is a
                    // Target-local Switch (ADR-0019).
                    CellResolution::Switch
                }
                Occupier::Untracked { .. } => {
                    if exact_direct {
                        CellResolution::Replace
                    } else {
                        resolution_of(cell_key)
                    }
                }
                Occupier::Empty => unreachable!("occupied entry has no occupier"),
            };
            if is_batch
                && matches!(occupier, Occupier::Untracked { .. })
                && resolution == CellResolution::Adopt
            {
                resolution = CellResolution::Skip;
            }
            if matches!(resolution, CellResolution::Skip) {
                (CellEligibility::Conflict, None, resolution, None)
            } else {
                let _ = current_row;
                (CellEligibility::Conflict, None, resolution, None)
            }
        }
    }
}

/// Intra-batch contention resolution (spec §4.9; §7.5; ADR-0019; #88):
/// When multiple skills in a batch share the same (target, Directory Identity)
/// entry path, at most one winner may proceed per Target. If no winner is
/// explicitly chosen (or multiple are chosen), all non-NoOp contenders remain
/// in Conflict with resolution Skip, without blocking other Ready cells.
fn resolve_batch_contention(cells: &mut [PlannedCell], resolutions: &[(String, CellResolution)]) {
    use std::collections::HashMap;

    let mut groups: HashMap<(String, PathBuf), Vec<usize>> = HashMap::new();
    for (idx, planned) in cells.iter().enumerate() {
        let key = (
            planned.cell.target_root_id.clone(),
            planned.cell.entry_path.clone(),
        );
        groups.entry(key).or_default().push(idx);
    }

    for ((_target_root_id, _entry_path), indices) in groups {
        if indices.len() <= 1 {
            continue;
        }

        // Multiple skills contend for the exact same target entry path.
        let mut chosen_winners = Vec::new();
        for &idx in &indices {
            let cell = &cells[idx].cell;
            if cell.eligibility == CellEligibility::Blocked {
                continue;
            }
            let user_res = resolutions
                .iter()
                .rev()
                .find(|(k, _)| k == &cell.cell_key)
                .map(|(_, r)| *r);
            if let Some(res) = user_res {
                if res == CellResolution::Replace || res == CellResolution::Switch {
                    chosen_winners.push((idx, res));
                }
            }
        }

        if chosen_winners.len() == 1 {
            let (winner_idx, winner_res) = chosen_winners[0];
            let winner = &mut cells[winner_idx];
            if winner.cell.occupancy == Occupier::Empty {
                winner.cell.eligibility = CellEligibility::Ready;
                winner.cell.resolution = CellResolution::Replace;
                winner.cell.detail = None;
            } else {
                winner.cell.eligibility = CellEligibility::Conflict;
                winner.cell.resolution = winner_res;
            }

            let winner_name = winner.cell.skill_name.clone();
            for &idx in &indices {
                if idx != winner_idx {
                    let loser = &mut cells[idx];
                    if loser.cell.eligibility != CellEligibility::Blocked {
                        loser.cell.eligibility = CellEligibility::Conflict;
                        loser.cell.resolution = CellResolution::Skip;
                        loser.cell.detail = Some(format!(
                            "contention with '{winner_name}'; not selected as winner"
                        ));
                    }
                }
            }
        } else if chosen_winners.is_empty() {
            // No winner chosen: existing NoOp cells stay NoOp, new candidates become Conflict / Skip.
            for &idx in &indices {
                let cell = &mut cells[idx];
                if cell.cell.eligibility != CellEligibility::Blocked
                    && cell.cell.eligibility != CellEligibility::NoOp
                {
                    cell.cell.eligibility = CellEligibility::Conflict;
                    cell.cell.resolution = CellResolution::Skip;
                    cell.cell.detail = Some(
                        "multiple skills in this batch share this Directory Identity on this Target; choose a winner"
                            .into(),
                    );
                }
            }
        } else {
            // Multiple winners chosen erroneously: all non-blocked contenders become Conflict / Skip.
            for &idx in &indices {
                let cell = &mut cells[idx];
                if cell.cell.eligibility != CellEligibility::Blocked {
                    cell.cell.eligibility = CellEligibility::Conflict;
                    cell.cell.resolution = CellResolution::Skip;
                    cell.cell.detail = Some(
                        "multiple winners selected for the same Target entry; only one winner may be selected"
                            .into(),
                    );
                }
            }
        }
    }
}

fn blocked_cell(
    _health: Health,
    source_gate: GateCode,
    availability: TargetGroupAvailability,
) -> (
    CellEligibility,
    Option<CellBlockedReason>,
    CellResolution,
    Option<String>,
) {
    let reason = match source_gate {
        GateCode::Mismatch => CellBlockedReason::SourceSnapshotMismatch,
        GateCode::Tombstoned => CellBlockedReason::TombstonedMember,
        GateCode::Ok => match availability {
            TargetGroupAvailability::Absent => CellBlockedReason::TargetAbsent,
            TargetGroupAvailability::Unavailable => CellBlockedReason::TargetUnavailable,
            TargetGroupAvailability::Available => CellBlockedReason::EntityBroken,
        },
    };
    (
        CellEligibility::Blocked,
        Some(reason),
        CellResolution::Skip,
        None,
    )
}

fn match_source_gate(
    source_gate: GateCode,
    availability: TargetGroupAvailability,
) -> Option<CellBlockedReason> {
    match source_gate {
        GateCode::Mismatch => Some(CellBlockedReason::SourceSnapshotMismatch),
        GateCode::Tombstoned => Some(CellBlockedReason::TombstonedMember),
        GateCode::Ok => match availability {
            TargetGroupAvailability::Absent => Some(CellBlockedReason::TargetAbsent),
            TargetGroupAvailability::Unavailable => Some(CellBlockedReason::TargetUnavailable),
            TargetGroupAvailability::Available => None,
        },
    }
}
