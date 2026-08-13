use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentId, AgentKind, SkillId, parse_skill_metadata, skill_identity_key,
};
use crate::core::write_gate::{PlanCheck, PlanTicket, WriteGate};
use crate::seams::activation_store::{
    ActivationObservation, ActivationRecord, ActivationStore, ActivationStoreError,
};
use crate::seams::adopt_store::LibraryConflict;
use crate::seams::agent_adapter::{
    AgentAdapterError, AgentAdapterRegistry, AgentEnableContext, AgentEnablePolicy,
};
use crate::seams::filesystem::{
    ActivationEntrySnapshot, ActivationReplaceJournal, ActivationReplacePhase,
    DirectoryFingerprint, FileSystem, FileSystemError, OccupantKind, OccupantSnapshot,
};

const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetActivation {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationPreview {
    pub plan_token: String,
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub skill_directory_name: String,
    pub agent_name: String,
    pub enabled: bool,
    pub kind: ActivationPlanKind,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub compatibility_warning: Option<crate::seams::agent_adapter::CompatibilityWarning>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationPlanKind {
    Enable,
    Disable,
    Repair,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationResult {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub desired_enabled: bool,
    pub observed_state: ActivationObservedState,
    pub snapshot_version: u64,
}

/// What occupies an Activation entry when Enable hits a Conflict. The sheet
/// uses this to offer Adopt existing item / Remove then replace / Cancel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationConflictDetails {
    pub skill_id: SkillId,
    pub agent_id: AgentId,
    pub entry_path: PathBuf,
    /// The target the Activation would point at (the Skill final entity).
    pub target_path: PathBuf,
    pub occupier: OccupierSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OccupierKind {
    RealDirectory,
    Symlink,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OccupierNotAdoptableReason {
    RegularFile,
    PointsAtManagedSkill,
    PointsAtThisSkill,
    NoReadableSkillMd,
    TargetUnresolvable,
    IdentityConflict { directory_name: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OccupierSummary {
    pub kind: OccupierKind,
    pub symlink_target: Option<PathBuf>,
    /// The resolved final entity when the occupier is a directory-like Skill.
    pub final_entity_path: Option<PathBuf>,
    pub directory_name: String,
    pub is_skill: bool,
    pub adoptable: bool,
    pub not_adoptable_reason: Option<OccupierNotAdoptableReason>,
}

/// Preview for Remove-then-replace: the occupant is moved to a journaled
/// backup before the Activation is created; Undo restores it while the result
/// window is open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationReplacePreview {
    pub plan_token: String,
    pub operation_id: String,
    pub skill_directory_name: String,
    pub agent_name: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub backup_path: PathBuf,
    pub occupant_kind: OccupierKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationReplaceUndoResult {
    pub undone: bool,
    pub error: Option<String>,
    pub snapshot_version: u64,
}

/// Identity conflict lookup used by conflict details: a Managed Skill may
/// already own the occupier's directory identity, which blocks Adopt.
/// Injected from the composition root; tests may omit it.
pub trait ActivationConflictChecker: Send + Sync {
    fn library_identity_conflict(
        &self,
        identity_key: &str,
    ) -> Result<Option<LibraryConflict>, ActivationStoreError>;
}

#[derive(Clone)]
struct PlannedActivation {
    request: SetActivation,
    kind: ActivationPlanKind,
    action: PlannedAction,
    agent_fingerprint: DirectoryFingerprint,
    library_fingerprint: DirectoryFingerprint,
    target_fingerprint: Option<DirectoryFingerprint>,
    entry_path: PathBuf,
    target_path: PathBuf,
    initial_entry: ActivationEntrySnapshot,
    gate_generation: u64,
    created_at: Instant,
}

#[derive(Clone)]
enum PlannedAction {
    Create,
    Remove,
}

#[derive(Clone)]
struct PlannedReplacement {
    request: SetActivation,
    operation_id: String,
    entry_path: PathBuf,
    target_path: PathBuf,
    backup_path: PathBuf,
    occupant: OccupantSnapshot,
    agent_fingerprint: DirectoryFingerprint,
    library_fingerprint: DirectoryFingerprint,
    target_fingerprint: DirectoryFingerprint,
    gate_generation: u64,
    created_at: Instant,
}

#[derive(Clone)]
struct AppliedReplacement {
    journal: ActivationReplaceJournal,
    skill_id: SkillId,
    agent_id: AgentId,
}

pub struct ActivationService {
    store: Arc<dyn ActivationStore>,
    filesystem: Arc<dyn FileSystem>,
    library_root: PathBuf,
    agent_adapters: Arc<dyn AgentAdapterRegistry>,
    conflict_checker: Option<Arc<dyn ActivationConflictChecker>>,
    plans: Mutex<HashMap<String, PlannedActivation>>,
    replace_plans: Mutex<HashMap<String, PlannedReplacement>>,
    applied_replacements: Mutex<HashMap<String, AppliedReplacement>>,
    next_plan_id: AtomicU64,
    plan_ttl: Duration,
    write_gate: Arc<WriteGate>,
}

impl ActivationService {
    pub fn new(
        store: Arc<dyn ActivationStore>,
        filesystem: Arc<dyn FileSystem>,
        library_root: PathBuf,
    ) -> Self {
        Self {
            store,
            filesystem,
            library_root,
            agent_adapters: Arc::new(ClaudeOnlyAgentAdapter),
            conflict_checker: None,
            plans: Mutex::new(HashMap::new()),
            replace_plans: Mutex::new(HashMap::new()),
            applied_replacements: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
            write_gate: Arc::new(WriteGate::open_for_tests()),
        }
    }

    pub fn with_agent_adapters(mut self, agent_adapters: Arc<dyn AgentAdapterRegistry>) -> Self {
        self.agent_adapters = agent_adapters;
        self
    }

    pub fn with_conflict_checker(mut self, checker: Arc<dyn ActivationConflictChecker>) -> Self {
        self.conflict_checker = Some(checker);
        self
    }

    pub fn with_plan_ttl(mut self, plan_ttl: Duration) -> Self {
        self.plan_ttl = plan_ttl;
        self
    }

    pub fn with_write_gate(mut self, write_gate: Arc<WriteGate>) -> Self {
        self.write_gate = write_gate;
        self
    }

    pub fn plan(&self, request: SetActivation) -> Result<ActivationPreview, ActivationError> {
        let kind = if request.enabled {
            ActivationPlanKind::Enable
        } else {
            ActivationPlanKind::Disable
        };
        self.plan_with_kind(request, kind)
    }

    pub fn plan_repair(
        &self,
        skill_id: SkillId,
        agent_id: AgentId,
    ) -> Result<ActivationPreview, ActivationError> {
        self.plan_with_kind(
            SetActivation {
                skill_id,
                agent_id,
                enabled: true,
            },
            ActivationPlanKind::Repair,
        )
    }

    fn plan_with_kind(
        &self,
        request: SetActivation,
        kind: ActivationPlanKind,
    ) -> Result<ActivationPreview, ActivationError> {
        self.ensure_writes_ready()?;
        let context = self
            .store
            .load(&request.skill_id, &request.agent_id)?
            .ok_or(ActivationError::NotFound)?;
        if kind == ActivationPlanKind::Repair && !context.desired_enabled {
            return Err(ActivationError::Validation(
                "Repair requires a desired Activation".into(),
            ));
        }
        validate_directory_name(&context.directory_name)?;
        let compatibility_warning = if request.enabled {
            let metadata = if context.agent_kind == AgentKind::ClaudePreset {
                None
            } else {
                Some(parse_skill_metadata(
                    &self
                        .filesystem
                        .read_skill_document(&context.final_entity_path)?,
                ))
            };
            self.agent_adapters
                .validate_enable(AgentEnableContext {
                    agent_kind: context.agent_kind,
                    directory_name: &context.directory_name,
                    frontmatter_name: metadata
                        .as_ref()
                        .and_then(|metadata| metadata.name.as_deref()),
                })?
                .compatibility_warning
        } else {
            None
        };
        let agent_root = self
            .filesystem
            .canonical_directory(&context.agent_skills_path)?;
        let library_root = self.validate_agent_path(&context.agent_id, &agent_root)?;
        let agent_fingerprint = self.filesystem.directory_fingerprint(&agent_root)?;
        let library_fingerprint = self.filesystem.directory_fingerprint(&library_root)?;
        let entry_path = agent_root.join(&context.directory_name);
        let initial_entry = self.filesystem.activation_snapshot(&entry_path)?;
        if kind == ActivationPlanKind::Repair {
            let expected_target_path = context.expected_target_path.clone().ok_or_else(|| {
                ActivationError::Validation(
                    "Repair requires a recorded expected Activation target".into(),
                )
            })?;
            match &initial_entry {
                ActivationEntrySnapshot::Missing => {
                    if !self
                        .filesystem
                        .skill_directory_is_readable(&expected_target_path)?
                    {
                        self.record_repair_observation(
                            &request,
                            ActivationObservedState::Dangling,
                        )?;
                        return Err(ActivationError::SourceUnavailable(expected_target_path));
                    }
                }
                ActivationEntrySnapshot::Other => {
                    self.record_repair_observation(&request, ActivationObservedState::Occupied)?;
                    return Err(ActivationError::Conflict(entry_path));
                }
                ActivationEntrySnapshot::Symlink { target } if target != &expected_target_path => {
                    self.record_repair_observation(
                        &request,
                        ActivationObservedState::TargetMismatch,
                    )?;
                    return Err(ActivationError::TargetMismatch(entry_path));
                }
                ActivationEntrySnapshot::Symlink { .. } => {
                    let observed_state = if self
                        .filesystem
                        .skill_directory_is_readable(&expected_target_path)?
                    {
                        ActivationObservedState::Present
                    } else {
                        ActivationObservedState::Dangling
                    };
                    self.record_repair_observation(&request, observed_state)?;
                    return if observed_state == ActivationObservedState::Present {
                        Err(ActivationError::Validation(
                            "Repair is no longer required".into(),
                        ))
                    } else {
                        Err(ActivationError::SourceUnavailable(expected_target_path))
                    };
                }
            }
        }
        let (action, target_path, target_fingerprint) = if request.enabled {
            let target_path = self
                .filesystem
                .canonical_directory(&context.final_entity_path)?;
            if initial_entry != ActivationEntrySnapshot::Missing {
                return Err(ActivationError::Conflict(entry_path));
            }
            let target_fingerprint = self.filesystem.directory_fingerprint(&target_path)?;
            (PlannedAction::Create, target_path, Some(target_fingerprint))
        } else {
            let expected_target = context.expected_target_path.ok_or_else(|| {
                ActivationError::Validation(
                    "Disable requires a recorded expected Activation target".into(),
                )
            })?;
            match &initial_entry {
                ActivationEntrySnapshot::Symlink { target } if target == &expected_target => {
                    (PlannedAction::Remove, expected_target, None)
                }
                _ => {
                    self.store.record(ActivationRecord {
                        skill_id: request.skill_id.clone(),
                        agent_id: request.agent_id.clone(),
                        desired_enabled: context.desired_enabled,
                        expected_entry_path: entry_path.clone(),
                        expected_target_path: expected_target,
                        observed_state: ActivationObservedState::TargetMismatch,
                    })?;
                    return Err(ActivationError::TargetMismatch(entry_path));
                }
            }
        };

        let plan_token = format!(
            "activation-plan-{}",
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        );
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ActivationError::Internal("Activation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        plans.insert(
            plan_token.clone(),
            PlannedActivation {
                request: request.clone(),
                kind,
                action,
                agent_fingerprint,
                library_fingerprint,
                target_fingerprint,
                entry_path: entry_path.clone(),
                target_path: target_path.clone(),
                initial_entry,
                gate_generation: self.write_gate.generation(),
                created_at: Instant::now(),
            },
        );

        Ok(ActivationPreview {
            plan_token,
            skill_id: request.skill_id.clone(),
            agent_id: request.agent_id.clone(),
            skill_directory_name: context.directory_name,
            agent_name: context.agent_name,
            enabled: request.enabled,
            kind,
            entry_path,
            target_path,
            compatibility_warning,
        })
    }

    pub fn apply(&self, plan_token: &str) -> Result<ActivationResult, ActivationError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ActivationError::Internal("Activation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        let plan = plans
            .remove(plan_token)
            .ok_or(ActivationError::PlanNotFound)?;
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ActivationError::PlanStale);
        }
        drop(plans);
        let current_context = self
            .store
            .load(&plan.request.skill_id, &plan.request.agent_id)?
            .ok_or(ActivationError::PlanStale)?;
        let current_agent_root = self
            .filesystem
            .canonical_directory(&current_context.agent_skills_path)
            .map_err(|_| ActivationError::PlanStale)?;
        let current_library_root = self
            .validate_agent_path(&plan.request.agent_id, &current_agent_root)
            .map_err(|_| ActivationError::PlanStale)?;
        if self
            .filesystem
            .directory_fingerprint(&current_agent_root)
            .map_err(|_| ActivationError::PlanStale)?
            != plan.agent_fingerprint
            || self
                .filesystem
                .directory_fingerprint(&current_library_root)
                .map_err(|_| ActivationError::PlanStale)?
                != plan.library_fingerprint
        {
            return Err(ActivationError::PlanStale);
        }
        self.ensure_activation_entry_unchanged(&plan)?;
        if plan.kind == ActivationPlanKind::Repair
            && !self
                .filesystem
                .skill_directory_is_readable(&plan.target_path)
                .map_err(|_| ActivationError::PlanStale)?
        {
            self.record_repair_observation(&plan.request, ActivationObservedState::Dangling)?;
            return Err(ActivationError::PlanStale);
        }
        if let Some(target_fingerprint) = &plan.target_fingerprint {
            let current_target = self
                .filesystem
                .directory_fingerprint(&plan.target_path)
                .map_err(|_| ActivationError::PlanStale)?;
            if &current_target != target_fingerprint {
                return Err(ActivationError::PlanStale);
            }
        }
        self.ensure_activation_entry_unchanged(&plan)?;

        match plan.action {
            PlannedAction::Create => {
                if let Err(error) = self
                    .filesystem
                    .create_activation(&plan.target_path, &plan.entry_path)
                {
                    if plan.kind == ActivationPlanKind::Repair
                        && matches!(
                            &error,
                            FileSystemError::Io { source, .. }
                                if source.kind() == std::io::ErrorKind::AlreadyExists
                        )
                    {
                        self.ensure_activation_entry_unchanged(&plan)?;
                    }
                    return Err(error.into());
                }
            }
            PlannedAction::Remove => self.filesystem.remove_activation(&plan.entry_path)?,
        }
        let observed_state = if plan.request.enabled {
            ActivationObservedState::Present
        } else {
            ActivationObservedState::Missing
        };
        let record = ActivationRecord {
            skill_id: plan.request.skill_id.clone(),
            agent_id: plan.request.agent_id.clone(),
            desired_enabled: plan.request.enabled,
            expected_entry_path: plan.entry_path.clone(),
            expected_target_path: plan.target_path.clone(),
            observed_state,
        };
        let snapshot_version = match self.store.record(record) {
            Ok(version) => version,
            Err(error) => {
                let compensation = match plan.action {
                    PlannedAction::Create => self.filesystem.remove_activation(&plan.entry_path),
                    PlannedAction::Remove => self
                        .filesystem
                        .create_activation(&plan.target_path, &plan.entry_path),
                };
                if let Err(compensation_error) = compensation {
                    return Err(ActivationError::RecoveryRequired {
                        state_error: error.to_string(),
                        compensation_error: compensation_error.to_string(),
                    });
                }
                return Err(error.into());
            }
        };

        Ok(ActivationResult {
            skill_id: plan.request.skill_id,
            agent_id: plan.request.agent_id,
            desired_enabled: plan.request.enabled,
            observed_state,
            snapshot_version,
        })
    }

    pub fn cancel(&self, plan_token: &str) -> Result<bool, ActivationError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ActivationError::Internal("Activation plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        Ok(plans.remove(plan_token).is_some())
    }

    /// Describe the content occupying an Activation entry so the Conflict
    /// sheet can offer Adopt existing item / Remove then replace / Cancel.
    /// Read-only: never writes the filesystem or the catalog.
    pub fn conflict_details(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<ActivationConflictDetails, ActivationError> {
        let context = self
            .store
            .load(skill_id, agent_id)?
            .ok_or(ActivationError::NotFound)?;
        validate_directory_name(&context.directory_name)?;
        let agent_root = self
            .filesystem
            .canonical_directory(&context.agent_skills_path)?;
        self.validate_agent_path(agent_id, &agent_root)?;
        let entry_path = agent_root.join(&context.directory_name);
        if matches!(
            self.filesystem.activation_snapshot(&entry_path)?,
            ActivationEntrySnapshot::Missing
        ) {
            return Err(ActivationError::Validation(
                "the Activation entry is no longer occupied".into(),
            ));
        }
        let occupant = self.filesystem.occupant_snapshot(&entry_path)?;
        let mut summary = OccupierSummary {
            kind: match &occupant.kind {
                OccupantKind::RealDirectory => OccupierKind::RealDirectory,
                OccupantKind::Symlink { .. } => OccupierKind::Symlink,
                OccupantKind::File { .. } => OccupierKind::File,
            },
            symlink_target: match &occupant.kind {
                OccupantKind::Symlink { target } => Some(target.clone()),
                _ => None,
            },
            final_entity_path: None,
            directory_name: context.directory_name.clone(),
            is_skill: false,
            adoptable: false,
            not_adoptable_reason: None,
        };
        match &occupant.kind {
            OccupantKind::File { .. } => {
                summary.not_adoptable_reason = Some(OccupierNotAdoptableReason::RegularFile);
            }
            OccupantKind::Symlink { .. } | OccupantKind::RealDirectory => {
                match self.filesystem.inspect_link_source(&entry_path) {
                    Ok(inspection) => {
                        summary.final_entity_path = Some(inspection.final_entity_path.clone());
                        summary.is_skill = self
                            .filesystem
                            .skill_directory_is_readable(&inspection.final_entity_path)?;
                        if summary.is_skill {
                            summary.adoptable = true;
                            let canonical_library =
                                self.filesystem.canonical_directory(&self.library_root)?;
                            if inspection.final_entity_path.starts_with(&canonical_library) {
                                summary.adoptable = false;
                                summary.not_adoptable_reason =
                                    Some(OccupierNotAdoptableReason::PointsAtManagedSkill);
                            } else {
                                let skill_target = self
                                    .filesystem
                                    .canonical_directory(&context.final_entity_path)?;
                                if inspection.final_entity_path == skill_target {
                                    summary.adoptable = false;
                                    summary.not_adoptable_reason =
                                        Some(OccupierNotAdoptableReason::PointsAtThisSkill);
                                }
                            }
                        } else {
                            summary.not_adoptable_reason =
                                Some(OccupierNotAdoptableReason::NoReadableSkillMd);
                        }
                    }
                    Err(_) => {
                        summary.not_adoptable_reason =
                            Some(OccupierNotAdoptableReason::TargetUnresolvable);
                    }
                }
            }
        }
        if summary.adoptable {
            if let Some(checker) = &self.conflict_checker {
                // §8.4: a candidate whose directory identity is already owned
                // by a Managed Skill (different entity) is not adoptable —
                // keep the Managed Skill; Remove or rename first. Because the
                // entry name always equals the Skill being enabled, this gate
                // blocks Adopt in the canonical Enable-conflict scenario; the
                // sheet explains the reason and offers Remove then replace.
                let conflict = checker
                    .library_identity_conflict(&skill_identity_key(&context.directory_name))?;
                if let Some(conflict) = conflict {
                    if conflict.final_entity_path
                        != summary.final_entity_path.clone().unwrap_or_default()
                    {
                        summary.adoptable = false;
                        summary.not_adoptable_reason =
                            Some(OccupierNotAdoptableReason::IdentityConflict {
                                directory_name: conflict.directory_name,
                            });
                    }
                }
            }
        }
        let target_path = self
            .filesystem
            .canonical_directory(&context.final_entity_path)?;
        Ok(ActivationConflictDetails {
            skill_id: skill_id.clone(),
            agent_id: agent_id.clone(),
            entry_path,
            target_path,
            occupier: summary,
        })
    }

    /// Plan Remove-then-replace: preflight the occupied entry, the Agent path
    /// policy and the target entity, and stage an in-memory plan. Nothing is
    /// written; Cancel leaves the filesystem and catalog untouched.
    pub fn plan_replace(
        &self,
        skill_id: &SkillId,
        agent_id: &AgentId,
    ) -> Result<ActivationReplacePreview, ActivationError> {
        self.ensure_writes_ready()?;
        let context = self
            .store
            .load(skill_id, agent_id)?
            .ok_or(ActivationError::NotFound)?;
        validate_directory_name(&context.directory_name)?;
        let agent_root = self
            .filesystem
            .canonical_directory(&context.agent_skills_path)?;
        let library_root = self.validate_agent_path(agent_id, &agent_root)?;
        let agent_fingerprint = self.filesystem.directory_fingerprint(&agent_root)?;
        let library_fingerprint = self.filesystem.directory_fingerprint(&library_root)?;
        let entry_path = agent_root.join(&context.directory_name);
        let snapshot = self.filesystem.activation_snapshot(&entry_path)?;
        if matches!(snapshot, ActivationEntrySnapshot::Missing) {
            return Err(ActivationError::Validation(
                "the Activation entry is no longer occupied".into(),
            ));
        }
        let occupant = self.filesystem.occupant_snapshot(&entry_path)?;
        let target_path = self
            .filesystem
            .canonical_directory(&context.final_entity_path)?;
        let target_fingerprint = self.filesystem.directory_fingerprint(&target_path)?;
        let operation_id = self.next_operation_id();
        let backup_path = library_root
            .join("operations")
            .join(&operation_id)
            .join("backup")
            .join(&context.directory_name);
        if !matches!(
            self.filesystem.activation_snapshot(&backup_path)?,
            ActivationEntrySnapshot::Missing
        ) {
            return Err(ActivationError::PlanStale);
        }
        let plan_token = format!(
            "activation-replace-plan-{}",
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        );
        let created_at = Instant::now();
        let mut plans = self.replace_plans.lock().map_err(|_| {
            ActivationError::Internal("Activation replace plan lock poisoned".into())
        })?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        plans.insert(
            plan_token.clone(),
            PlannedReplacement {
                request: SetActivation {
                    skill_id: skill_id.clone(),
                    agent_id: agent_id.clone(),
                    enabled: true,
                },
                operation_id: operation_id.clone(),
                entry_path: entry_path.clone(),
                target_path: target_path.clone(),
                backup_path: backup_path.clone(),
                occupant: occupant.clone(),
                agent_fingerprint,
                library_fingerprint,
                target_fingerprint,
                gate_generation: self.write_gate.generation(),
                created_at,
            },
        );
        Ok(ActivationReplacePreview {
            plan_token,
            operation_id,
            skill_directory_name: context.directory_name,
            agent_name: context.agent_name,
            entry_path,
            target_path,
            backup_path,
            occupant_kind: match occupant.kind {
                OccupantKind::RealDirectory => OccupierKind::RealDirectory,
                OccupantKind::Symlink { .. } => OccupierKind::Symlink,
                OccupantKind::File { .. } => OccupierKind::File,
            },
        })
    }

    /// Apply Remove-then-replace with a durable journal: move the occupant to
    /// backup, create the Activation, commit the desired state. A crash
    /// between steps is repaired at next startup; Undo restores the occupant
    /// while the result window is open.
    pub fn apply_replace(&self, plan_token: &str) -> Result<ActivationResult, ActivationError> {
        self.ensure_writes_ready()?;
        let mut plans = self.replace_plans.lock().map_err(|_| {
            ActivationError::Internal("Activation replace plan lock poisoned".into())
        })?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        let plan = plans
            .remove(plan_token)
            .ok_or(ActivationError::PlanNotFound)?;
        if self.write_gate.check_plan(PlanTicket {
            generation: plan.gate_generation,
        }) == PlanCheck::Stale
        {
            return Err(ActivationError::PlanStale);
        }
        drop(plans);
        let context = self
            .store
            .load(&plan.request.skill_id, &plan.request.agent_id)?
            .ok_or(ActivationError::PlanStale)?;
        let current_agent_root = self
            .filesystem
            .canonical_directory(&context.agent_skills_path)
            .map_err(|_| ActivationError::PlanStale)?;
        let current_library_root = self
            .validate_agent_path(&plan.request.agent_id, &current_agent_root)
            .map_err(|_| ActivationError::PlanStale)?;
        if self
            .filesystem
            .directory_fingerprint(&current_agent_root)
            .map_err(|_| ActivationError::PlanStale)?
            != plan.agent_fingerprint
            || self
                .filesystem
                .directory_fingerprint(&current_library_root)
                .map_err(|_| ActivationError::PlanStale)?
                != plan.library_fingerprint
        {
            return Err(ActivationError::PlanStale);
        }
        if self
            .filesystem
            .occupant_snapshot(&plan.entry_path)
            .map_err(|_| ActivationError::PlanStale)?
            != plan.occupant
        {
            return Err(ActivationError::PlanStale);
        }
        if self
            .filesystem
            .directory_fingerprint(&plan.target_path)
            .map_err(|_| ActivationError::PlanStale)?
            != plan.target_fingerprint
        {
            return Err(ActivationError::PlanStale);
        }
        let journal = ActivationReplaceJournal {
            version: 1,
            operation_id: plan.operation_id.clone(),
            phase: ActivationReplacePhase::Applying,
            skill_id: plan.request.skill_id.0.clone(),
            agent_id: plan.request.agent_id.0.clone(),
            entry_path: plan.entry_path.clone(),
            target_path: plan.target_path.clone(),
            backup_path: plan.backup_path.clone(),
            occupant: plan.occupant.clone(),
        };
        self.filesystem
            .write_activation_replace_journal(&self.library_root, &journal)?;
        if let Err(error) = self.filesystem.move_occupant_to_backup(
            &plan.entry_path,
            &plan.backup_path,
            &self.library_root,
            &plan.occupant,
        ) {
            let state = self.filesystem.activation_snapshot(&plan.entry_path)?;
            match state {
                ActivationEntrySnapshot::Other => {
                    // Nothing moved; leave the entry untouched and close the
                    // journal so startup recovery does not misread it.
                    self.close_replace_journal(&journal)?;
                }
                ActivationEntrySnapshot::Missing => {
                    self.restore_after_failed_replace(&plan, &journal, &error.to_string())?;
                }
                ActivationEntrySnapshot::Symlink { .. } => {
                    let unchanged = self
                        .filesystem
                        .occupant_snapshot(&plan.entry_path)
                        .map_err(|_| ActivationError::PlanStale)?
                        == plan.occupant;
                    if unchanged {
                        // The occupant is still at the entry; nothing moved.
                        self.close_replace_journal(&journal)?;
                    } else {
                        return Err(ActivationError::RecoveryRequired {
                            state_error: error.to_string(),
                            compensation_error: "the entry changed while the occupant was moved"
                                .into(),
                        });
                    }
                }
            }
            return Err(error.into());
        }
        if let Err(error) = self
            .filesystem
            .create_activation(&plan.target_path, &plan.entry_path)
        {
            self.restore_after_failed_replace(&plan, &journal, &error.to_string())?;
            return Err(error.into());
        }
        let record = ActivationRecord {
            skill_id: plan.request.skill_id.clone(),
            agent_id: plan.request.agent_id.clone(),
            desired_enabled: true,
            expected_entry_path: plan.entry_path.clone(),
            expected_target_path: plan.target_path.clone(),
            observed_state: ActivationObservedState::Present,
        };
        match self.store.record(record) {
            Ok(snapshot_version) => {
                let mut committed = journal.clone();
                committed.phase = ActivationReplacePhase::Committed;
                self.filesystem
                    .write_activation_replace_journal(&self.library_root, &committed)?;
                self.applied_replacements
                    .lock()
                    .map_err(|_| {
                        ActivationError::Internal("Activation replace applied lock poisoned".into())
                    })?
                    .insert(
                        plan.operation_id.clone(),
                        AppliedReplacement {
                            journal: committed,
                            skill_id: plan.request.skill_id.clone(),
                            agent_id: plan.request.agent_id.clone(),
                        },
                    );
                Ok(ActivationResult {
                    skill_id: plan.request.skill_id,
                    agent_id: plan.request.agent_id,
                    desired_enabled: true,
                    observed_state: ActivationObservedState::Present,
                    snapshot_version,
                })
            }
            Err(error) => {
                let remove = self
                    .filesystem
                    .remove_activation(&plan.entry_path)
                    .or_else(|error| match error {
                        FileSystemError::Io { source, .. }
                            if source.kind() == std::io::ErrorKind::NotFound =>
                        {
                            Ok(())
                        }
                        other => Err(other),
                    });
                let restore = remove.and_then(|_| {
                    self.filesystem.restore_occupant_from_backup(
                        &plan.backup_path,
                        &plan.entry_path,
                        &self.library_root,
                        &plan.occupant,
                    )
                });
                if let Err(compensation) = restore {
                    return Err(ActivationError::RecoveryRequired {
                        state_error: error.to_string(),
                        compensation_error: compensation.to_string(),
                    });
                }
                self.close_replace_journal(&journal)?;
                Err(error.into())
            }
        }
    }

    /// Discard a planned Replace without touching the filesystem: the journal
    /// is only written at Apply time, so Cancel leaves zero changes.
    pub fn cancel_replace(&self, plan_token: &str) -> Result<bool, ActivationError> {
        self.ensure_writes_ready()?;
        let mut plans = self.replace_plans.lock().map_err(|_| {
            ActivationError::Internal("Activation replace plan lock poisoned".into())
        })?;
        plans.retain(|_, plan| plan.created_at.elapsed() < self.plan_ttl);
        Ok(plans.remove(plan_token).is_some())
    }

    /// Undo a completed Replace while its result window is open: restore the
    /// occupant to its entry and record the Skill as disabled. The entry is
    /// re-preflighted; external content at the entry is never overwritten.
    pub fn undo_replace(
        &self,
        operation_id: &str,
    ) -> Result<ActivationReplaceUndoResult, ActivationError> {
        self.ensure_writes_ready()?;
        let applied = self
            .applied_replacements
            .lock()
            .map_err(|_| {
                ActivationError::Internal("Activation replace applied lock poisoned".into())
            })?
            .remove(operation_id)
            .ok_or(ActivationError::PlanNotFound)?;
        let mut journal = applied.journal.clone();
        journal.phase = ActivationReplacePhase::Undoing;
        self.filesystem
            .write_activation_replace_journal(&self.library_root, &journal)?;
        match self.filesystem.activation_snapshot(&journal.entry_path)? {
            ActivationEntrySnapshot::Missing => {}
            ActivationEntrySnapshot::Symlink { target } if target == journal.target_path => {
                self.filesystem.remove_activation(&journal.entry_path)?;
            }
            _ => {
                let mut committed = journal;
                committed.phase = ActivationReplacePhase::Committed;
                self.filesystem
                    .write_activation_replace_journal(&self.library_root, &committed)?;
                return Ok(ActivationReplaceUndoResult {
                    undone: false,
                    error: Some(
                        "the Activation entry was replaced by external content; the original item could not be restored"
                            .into(),
                    ),
                    snapshot_version: 0,
                });
            }
        }
        if let Err(error) = self.filesystem.restore_occupant_from_backup(
            &journal.backup_path,
            &journal.entry_path,
            &self.library_root,
            &journal.occupant,
        ) {
            return Err(ActivationError::RecoveryRequired {
                state_error: "the occupant could not be restored".into(),
                compensation_error: error.to_string(),
            });
        }
        let record = ActivationRecord {
            skill_id: applied.skill_id.clone(),
            agent_id: applied.agent_id.clone(),
            desired_enabled: false,
            expected_entry_path: journal.entry_path.clone(),
            expected_target_path: journal.target_path.clone(),
            observed_state: ActivationObservedState::Occupied,
        };
        match self.store.record(record) {
            Ok(snapshot_version) => {
                self.filesystem
                    .finish_activation_replace_journal(&self.library_root, operation_id)?;
                Ok(ActivationReplaceUndoResult {
                    undone: true,
                    error: None,
                    snapshot_version,
                })
            }
            Err(error) => {
                // The filesystem restore succeeded; the journal stays in the
                // Undoing phase so startup recovery completes and preserves
                // the backup. The next health check observes the occupied
                // entry honestly.
                Err(error.into())
            }
        }
    }

    /// Close a Replace result window: discard the committed backup and
    /// archive the journal. The operation can no longer be undone.
    pub fn finalize_replace(&self, operation_id: &str) -> Result<(), ActivationError> {
        self.ensure_writes_ready()?;
        let applied = self
            .applied_replacements
            .lock()
            .map_err(|_| {
                ActivationError::Internal("Activation replace applied lock poisoned".into())
            })?
            .remove(operation_id)
            .ok_or(ActivationError::PlanNotFound)?;
        self.filesystem.discard_replace_backup(
            &applied.journal.backup_path,
            &self.library_root,
            &applied.journal.occupant,
        )?;
        self.filesystem
            .finish_activation_replace_journal(&self.library_root, operation_id)?;
        Ok(())
    }

    fn next_operation_id(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        format!(
            "activation-replace-{}-{}",
            nanos,
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Mark the journal Committed and archive it after a compensation left
    /// the filesystem and catalog consistent (nothing to recover).
    fn close_replace_journal(
        &self,
        journal: &ActivationReplaceJournal,
    ) -> Result<(), ActivationError> {
        let mut committed = journal.clone();
        committed.phase = ActivationReplacePhase::Committed;
        self.filesystem
            .write_activation_replace_journal(&self.library_root, &committed)?;
        self.filesystem
            .finish_activation_replace_journal(&self.library_root, &journal.operation_id)
            .map_err(ActivationError::from)
    }

    /// Compensate a failed Replace move/create: restore the occupant (or
    /// leave it in place if the move never happened) and close the journal.
    fn restore_after_failed_replace(
        &self,
        plan: &PlannedReplacement,
        journal: &ActivationReplaceJournal,
        state_error: &str,
    ) -> Result<(), ActivationError> {
        let restore = self.filesystem.restore_occupant_from_backup(
            &plan.backup_path,
            &plan.entry_path,
            &self.library_root,
            &plan.occupant,
        );
        if let Err(compensation) = restore {
            return Err(ActivationError::RecoveryRequired {
                state_error: state_error.to_owned(),
                compensation_error: compensation.to_string(),
            });
        }
        self.close_replace_journal(journal)
    }

    fn ensure_writes_ready(&self) -> Result<(), ActivationError> {
        if self.write_gate.is_product_write_open() {
            Ok(())
        } else {
            Err(ActivationError::RecoveryInProgress)
        }
    }

    fn record_repair_observation(
        &self,
        request: &SetActivation,
        observed_state: ActivationObservedState,
    ) -> Result<(), ActivationError> {
        self.store.record_observation(&ActivationObservation {
            skill_id: request.skill_id.clone(),
            agent_id: request.agent_id.clone(),
            observed_state,
        })?;
        Ok(())
    }

    fn ensure_activation_entry_unchanged(
        &self,
        plan: &PlannedActivation,
    ) -> Result<(), ActivationError> {
        let current_entry = self.filesystem.activation_snapshot(&plan.entry_path)?;
        if current_entry == plan.initial_entry {
            return Ok(());
        }
        if plan.kind != ActivationPlanKind::Repair {
            return Err(ActivationError::PlanStale);
        }

        match current_entry {
            ActivationEntrySnapshot::Other => {
                self.record_repair_observation(&plan.request, ActivationObservedState::Occupied)?;
                Err(ActivationError::Conflict(plan.entry_path.clone()))
            }
            ActivationEntrySnapshot::Symlink { target } if target != plan.target_path => {
                self.record_repair_observation(
                    &plan.request,
                    ActivationObservedState::TargetMismatch,
                )?;
                Err(ActivationError::TargetMismatch(plan.entry_path.clone()))
            }
            ActivationEntrySnapshot::Symlink { .. } => {
                let observed_state = if self
                    .filesystem
                    .skill_directory_is_readable(&plan.target_path)?
                {
                    ActivationObservedState::Present
                } else {
                    ActivationObservedState::Dangling
                };
                self.record_repair_observation(&plan.request, observed_state)?;
                if observed_state == ActivationObservedState::Present {
                    Err(ActivationError::Validation(
                        "Repair is no longer required".into(),
                    ))
                } else {
                    Err(ActivationError::SourceUnavailable(plan.target_path.clone()))
                }
            }
            ActivationEntrySnapshot::Missing => Err(ActivationError::PlanStale),
        }
    }

    fn validate_agent_path(
        &self,
        selected_agent_id: &AgentId,
        selected_path: &Path,
    ) -> Result<PathBuf, ActivationError> {
        let library_root = self.filesystem.canonical_directory(&self.library_root)?;
        if paths_overlap(selected_path, &library_root) {
            return Err(ActivationError::PathOverlap);
        }

        for configured in self.store.configured_agent_paths()? {
            if configured.agent_id == *selected_agent_id {
                continue;
            }
            let other_path = self
                .filesystem
                .normalize_configured_path(&configured.skills_path)?;
            if paths_overlap(selected_path, &other_path) {
                return Err(ActivationError::PathOverlap);
            }
        }
        Ok(library_root)
    }
}

struct ClaudeOnlyAgentAdapter;

impl AgentAdapterRegistry for ClaudeOnlyAgentAdapter {
    fn validate_enable(
        &self,
        context: AgentEnableContext<'_>,
    ) -> Result<AgentEnablePolicy, AgentAdapterError> {
        if context.agent_kind == AgentKind::ClaudePreset {
            Ok(AgentEnablePolicy::default())
        } else {
            Err(AgentAdapterError(
                "this ActivationService requires an AgentAdapter for non-Claude Agents".into(),
            ))
        }
    }
}

fn validate_directory_name(directory_name: &str) -> Result<(), ActivationError> {
    let mut components = Path::new(directory_name).components();
    if directory_name.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(ActivationError::Validation(
            "Skill directory identity must be one path component".into(),
        ));
    }
    Ok(())
}

fn paths_overlap(first: &Path, second: &Path) -> bool {
    first.starts_with(second) || second.starts_with(first)
}

#[derive(Debug, Error)]
pub enum ActivationError {
    #[error("the Managed Skill or Agent was not found")]
    NotFound,
    #[error("Activation validation failed: {0}")]
    Validation(String),
    #[error("the Activation path conflicts with existing content: {}", .0.display())]
    Conflict(PathBuf),
    #[error("the Skill entity is unreadable or does not contain SKILL.md: {}", .0.display())]
    SourceUnavailable(PathBuf),
    #[error("the Activation no longer matches its recorded target: {}", .0.display())]
    TargetMismatch(PathBuf),
    #[error("the Agent path overlaps the Library or another Agent path")]
    PathOverlap,
    #[error("the Activation plan no longer matches the filesystem")]
    PlanStale,
    #[error("the Activation plan was not found or has expired")]
    PlanNotFound,
    #[error("startup recovery is still in progress")]
    RecoveryInProgress,
    #[error(
        "Activation state failed and filesystem compensation also failed; recovery is required: state={state_error}; compensation={compensation_error}"
    )]
    RecoveryRequired {
        state_error: String,
        compensation_error: String,
    },
    #[error(transparent)]
    FileSystem(#[from] FileSystemError),
    #[error(transparent)]
    Store(#[from] ActivationStoreError),
    #[error(transparent)]
    AgentAdapter(#[from] AgentAdapterError),
    #[error("internal Activation error: {0}")]
    Internal(String),
}
