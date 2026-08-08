use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::core::domain::{
    ActivationObservedState, AgentId, AgentKind, SkillId, parse_skill_metadata,
};
use crate::seams::activation_store::{
    ActivationObservation, ActivationRecord, ActivationStore, ActivationStoreError,
};
use crate::seams::agent_adapter::{
    AgentAdapterError, AgentAdapterRegistry, AgentEnableContext, AgentEnablePolicy,
};
use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileSystem, FileSystemError,
};
use crate::seams::recovery::RecoveryGate;

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
    pub skill_directory_name: String,
    pub agent_name: String,
    pub enabled: bool,
    pub kind: ActivationPlanKind,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub compatibility_warning: Option<String>,
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
    created_at: Instant,
}

#[derive(Clone)]
enum PlannedAction {
    Create,
    Remove,
}

pub struct ActivationService {
    store: Arc<dyn ActivationStore>,
    filesystem: Arc<dyn FileSystem>,
    library_root: PathBuf,
    agent_adapters: Arc<dyn AgentAdapterRegistry>,
    plans: Mutex<HashMap<String, PlannedActivation>>,
    next_plan_id: AtomicU64,
    plan_ttl: Duration,
    recovery_gate: Arc<RecoveryGate>,
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
            plans: Mutex::new(HashMap::new()),
            next_plan_id: AtomicU64::new(1),
            plan_ttl: DEFAULT_PLAN_TTL,
            recovery_gate: Arc::new(RecoveryGate::ready()),
        }
    }

    pub fn with_agent_adapters(mut self, agent_adapters: Arc<dyn AgentAdapterRegistry>) -> Self {
        self.agent_adapters = agent_adapters;
        self
    }

    pub fn with_plan_ttl(mut self, plan_ttl: Duration) -> Self {
        self.plan_ttl = plan_ttl;
        self
    }

    pub fn with_recovery_gate(mut self, recovery_gate: Arc<RecoveryGate>) -> Self {
        self.recovery_gate = recovery_gate;
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
                created_at: Instant::now(),
            },
        );

        Ok(ActivationPreview {
            plan_token,
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

    fn ensure_writes_ready(&self) -> Result<(), ActivationError> {
        if self.recovery_gate.writes_are_ready() {
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
