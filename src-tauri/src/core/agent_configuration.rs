use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use thiserror::Error;

pub use crate::core::domain::Compatibility;
use crate::core::domain::{agent_name_identity_key, configured_path_identity_key};
use crate::core::write_gate::{
    HomeWriteContext, PlanCheck, PlanTicket, ProductWriteGuard, WriteGate, WriteGateError,
    WriteGateState,
};
use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootInspection,
    CreatedAgentTargetDirectory,
};
use crate::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, AgentConfigurationWrite, AgentRootWrite,
    StoredAgentConfiguration, StoredGlobalSkillRoot,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentConfigurationOrigin {
    Preset,
    Custom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRootRole {
    ScanOnly,
    ActivationTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentConfigurationPlanKind {
    Create,
    Edit,
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRootDraft {
    pub configured_path: PathBuf,
    pub role: AgentRootRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationDraft {
    pub preset_key: Option<String>,
    pub name: String,
    pub roots: Vec<AgentRootDraft>,
    pub project_skills_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationRoot {
    pub root_id: String,
    pub configured_path: PathBuf,
    pub path_identity_key: String,
    pub role: AgentRootRole,
    pub consumer_agent_ids: Vec<String>,
    pub activation_skill_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfiguration {
    pub agent_id: String,
    pub origin: AgentConfigurationOrigin,
    pub preset_key: Option<String>,
    pub name: String,
    pub compatibility: Compatibility,
    pub project_skills_dir: Option<PathBuf>,
    pub roots: Vec<AgentConfigurationRoot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPreset {
    pub preset_key: String,
    pub name: String,
    pub compatibility: Compatibility,
    pub roots: Vec<PathBuf>,
    pub activation_target: PathBuf,
    pub project_skills_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentManagementSnapshot {
    pub generation: u64,
    pub configurations: Vec<AgentConfiguration>,
    pub presets: Vec<AgentPreset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationPlan {
    pub plan_token: String,
    pub kind: AgentConfigurationPlanKind,
    pub configuration: Option<AgentConfiguration>,
    pub target_will_be_created: bool,
    pub blocking_activation_skill_ids: Vec<String>,
    pub retained_activation_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConfigurationApplyResult {
    pub agent_id: String,
    pub generation: u64,
    pub deleted: bool,
    /// Target Root ids affected by the apply: the new Target of a Create/
    /// Edit plus (for Edit/Delete) the previously referenced Target. The
    /// caller schedules Target-scoped Activation health for exactly these
    /// (spec §4.10; ADR-0020: only affected Targets).
    pub affected_target_root_ids: Vec<String>,
}

#[derive(Clone)]
pub struct PresetRegistry {
    presets: Vec<AgentPreset>,
}

impl PresetRegistry {
    pub fn system() -> Self {
        let omp_root = std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .map(|path| path.join("skills"))
            .unwrap_or_else(|| PathBuf::from("~/.omp/agent/skills"));
        let codex_legacy = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .map(|path| path.join("skills"))
            .unwrap_or_else(|| PathBuf::from("~/.codex/skills"));
        let mut presets = vec![
            preset(
                "omp",
                "omp",
                vec![omp_root.clone()],
                omp_root,
                ".omp/skills",
            ),
            preset(
                "claude-code",
                "Claude Code",
                vec![PathBuf::from("~/.claude/skills")],
                PathBuf::from("~/.claude/skills"),
                ".claude/skills",
            ),
            preset(
                "codex",
                "Codex",
                vec![codex_legacy.clone()],
                codex_legacy,
                ".codex/skills",
            ),
            preset(
                "gemini-cli",
                "Gemini CLI",
                vec![
                    PathBuf::from("~/.gemini/skills"),
                    PathBuf::from("~/.agents/skills"),
                ],
                PathBuf::from("~/.gemini/skills"),
                ".gemini/skills",
            ),
            preset(
                "cursor",
                "Cursor",
                vec![
                    PathBuf::from("~/.cursor/skills"),
                    PathBuf::from("~/.agents/skills"),
                    PathBuf::from("~/.claude/skills"),
                    PathBuf::from("~/.codex/skills"),
                ],
                PathBuf::from("~/.cursor/skills"),
                ".cursor/skills",
            ),
            preset(
                "opencode",
                "opencode",
                vec![
                    PathBuf::from("~/.config/opencode/skill"),
                    PathBuf::from("~/.config/opencode/skills"),
                    PathBuf::from("~/.claude/skills"),
                    PathBuf::from("~/.agents/skills"),
                ],
                PathBuf::from("~/.config/opencode/skills"),
                ".opencode/skills",
            ),
            preset(
                "github-copilot",
                "GitHub Copilot",
                vec![
                    PathBuf::from("~/.copilot/skills"),
                    PathBuf::from("~/.agents/skills"),
                ],
                PathBuf::from("~/.copilot/skills"),
                ".github/skills",
            ),
            preset(
                "general",
                "General",
                vec![PathBuf::from("~/.agents/skills")],
                PathBuf::from("~/.agents/skills"),
                ".agents/skills",
            ),
            preset(
                "windsurf",
                "Windsurf",
                vec![
                    PathBuf::from("~/.codeium/windsurf/skills"),
                    PathBuf::from("~/.agents/skills"),
                    PathBuf::from("~/.claude/skills"),
                ],
                PathBuf::from("~/.codeium/windsurf/skills"),
                ".windsurf/skills",
            ),
        ];
        for preset in &mut presets {
            if preset.preset_key != "general" {
                preset
                    .roots
                    .retain(|root| root != &PathBuf::from("~/.agents/skills"));
            }
            let mut seen = BTreeSet::new();
            preset
                .roots
                .retain(|root| seen.insert(root.to_string_lossy().into_owned()));
        }
        Self { presets }
    }

    pub fn presets(&self) -> &[AgentPreset] {
        &self.presets
    }

    pub fn get(&self, preset_key: &str) -> Option<&AgentPreset> {
        // Older Zed configurations used only the shared roots. Keep their
        // validation/edit path valid without offering a duplicate template.
        let preset_key = if preset_key == "zed" {
            "general"
        } else {
            preset_key
        };
        self.presets
            .iter()
            .find(|preset| preset.preset_key == preset_key)
    }

    #[cfg(test)]
    pub(crate) fn from_presets(presets: Vec<AgentPreset>) -> Self {
        Self { presets }
    }
}

fn preset(
    preset_key: &str,
    name: &str,
    roots: Vec<PathBuf>,
    activation_target: PathBuf,
    project_skills_dir: &str,
) -> AgentPreset {
    AgentPreset {
        preset_key: preset_key.into(),
        name: name.into(),
        compatibility: Compatibility::Verified,
        roots,
        activation_target,
        project_skills_dir: PathBuf::from(project_skills_dir),
    }
}

#[derive(Debug, Error)]
pub enum AgentConfigurationError {
    #[error("Agent Configuration writes are unavailable")]
    WriteGateClosed,
    #[error("the Agent Configuration name must contain 1 to 80 characters")]
    InvalidName,
    #[error("Agent Configuration name '{name}' is already in use")]
    NameConflict { name: String },
    #[error("Agent Preset '{preset_key}' does not exist")]
    PresetNotFound { preset_key: String },
    #[error("an Agent Configuration requires at least one Global Skills Root")]
    RootRequired,
    #[error("an Agent Configuration requires exactly one Activation Target")]
    ActivationTargetRequired,
    #[error("Global Skills Root '{path}' is duplicated")]
    DuplicateRoot { path: String },
    #[error("Global Skills Roots '{path}' and '{conflicting_path}' overlap")]
    RootOverlap {
        path: String,
        conflicting_path: String,
    },
    #[error("Global Skills Root '{path}' overlaps the Bound Home")]
    HomeOverlap { path: String },
    #[error("Global Skills Root '{path}' is not a safe configured path")]
    InvalidRoot { path: String },
    #[error("Agent Activation Target '{path}' is not writable")]
    TargetNotWritable { path: String },
    #[error("Agent Activation Target '{path}' is a system, builtin or cache root")]
    TargetNotAllowed { path: String },
    #[error("project_skills_dir is not a safe repository-relative directory")]
    InvalidProjectSkillsDir,
    #[error("Agent Configuration '{agent_id}' was not found")]
    NotFound { agent_id: String },
    #[error("the Agent Configuration plan is stale")]
    PlanStale,
    #[error("the last Target reference owns Activations")]
    TargetInUse { skill_ids: Vec<String> },
    #[error("Agent Configuration storage failed: {0}")]
    Store(String),
    #[error("Agent Configuration filesystem failed: {0}")]
    FileSystem(String),
    #[error("an operation-created Target requires recovery: {0}")]
    RecoveryRequired(String),
}

#[derive(Clone)]
struct PlannedRoot {
    write: AgentRootWrite,
    inspection: AgentRootInspection,
}

#[derive(Clone)]
struct PendingPlan {
    ticket: PlanTicket,
    write_context: HomeWriteContext,
    snapshot_version: u64,
    kind: AgentConfigurationPlanKind,
    change: AgentConfigurationStoreChange,
    roots: Vec<PlannedRoot>,
    blocking_activation_skill_ids: Vec<String>,
}

pub struct AgentConfigurationService {
    store: Arc<dyn AgentConfigurationStore>,
    filesystem: Arc<dyn AgentConfigurationFileSystem>,
    write_gate: Arc<WriteGate>,
    presets: PresetRegistry,
    plans: Mutex<HashMap<String, PendingPlan>>,
}

impl AgentConfigurationService {
    pub fn new(
        store: Arc<dyn AgentConfigurationStore>,
        filesystem: Arc<dyn AgentConfigurationFileSystem>,
        write_gate: Arc<WriteGate>,
        presets: PresetRegistry,
    ) -> Self {
        Self {
            store,
            filesystem,
            write_gate,
            presets,
            plans: Mutex::new(HashMap::new()),
        }
    }

    pub fn snapshot(&self) -> Result<AgentManagementSnapshot, AgentConfigurationError> {
        let snapshot = self
            .store
            .agent_configuration_snapshot()
            .map_err(map_store_error)?;
        Ok(self.render_snapshot(snapshot))
    }

    pub fn plan_create(
        &self,
        draft: AgentConfigurationDraft,
    ) -> Result<AgentConfigurationPlan, AgentConfigurationError> {
        let agent_id = self.new_id()?;
        self.plan_upsert(AgentConfigurationPlanKind::Create, agent_id, draft)
    }

    pub fn plan_edit(
        &self,
        agent_id: &str,
        draft: AgentConfigurationDraft,
    ) -> Result<AgentConfigurationPlan, AgentConfigurationError> {
        self.plan_upsert(AgentConfigurationPlanKind::Edit, agent_id.to_owned(), draft)
    }

    pub fn plan_delete(
        &self,
        agent_id: &str,
    ) -> Result<AgentConfigurationPlan, AgentConfigurationError> {
        self.require_open_gate()?;
        let write_context = self.capture_write_context()?;
        let snapshot = self
            .store
            .agent_configuration_snapshot()
            .map_err(map_store_error)?;
        let configuration = snapshot
            .configurations
            .iter()
            .find(|configuration| configuration.agent_id == agent_id)
            .ok_or_else(|| AgentConfigurationError::NotFound {
                agent_id: agent_id.to_owned(),
            })?;
        let target = target_root(configuration).ok_or_else(|| {
            AgentConfigurationError::Store(
                "persisted Agent Configuration has no Activation Target".into(),
            )
        })?;
        let root = snapshot
            .roots
            .iter()
            .find(|root| root.root_id == target.root_id)
            .ok_or_else(|| {
                AgentConfigurationError::Store("persisted Target Root is missing".into())
            })?;
        let last_reference = root.consumer_agent_ids.len() == 1;
        let blocking = if last_reference {
            root.activation_skill_ids.clone()
        } else {
            Vec::new()
        };
        let retained_activation_count = if last_reference {
            0
        } else {
            root.activation_skill_ids.len()
        };
        let plan_token = self.new_id()?;
        let plan = PendingPlan {
            ticket: PlanTicket {
                generation: write_context.generation,
            },
            write_context,
            snapshot_version: snapshot.snapshot_version,
            kind: AgentConfigurationPlanKind::Delete,
            change: AgentConfigurationStoreChange::Delete {
                agent_id: agent_id.to_owned(),
            },
            roots: Vec::new(),
            blocking_activation_skill_ids: blocking.clone(),
        };
        self.plans
            .lock()
            .map_err(|_| AgentConfigurationError::Store("plan lock is poisoned".into()))?
            .insert(plan_token.clone(), plan);
        Ok(AgentConfigurationPlan {
            plan_token,
            kind: AgentConfigurationPlanKind::Delete,
            configuration: None,
            target_will_be_created: false,
            blocking_activation_skill_ids: blocking,
            retained_activation_count,
        })
    }

    pub fn apply(
        &self,
        plan_token: &str,
    ) -> Result<AgentConfigurationApplyResult, AgentConfigurationError> {
        self.require_open_gate()?;
        let plan = self
            .plans
            .lock()
            .map_err(|_| AgentConfigurationError::Store("plan lock is poisoned".into()))?
            .get(plan_token)
            .cloned()
            .ok_or(AgentConfigurationError::PlanStale)?;
        if self.write_gate.check_plan(plan.ticket) != PlanCheck::Current {
            return Err(AgentConfigurationError::PlanStale);
        }
        self.write_gate
            .validate_open_context(&plan.write_context)
            .map_err(|error| match error {
                WriteGateError::Stale => AgentConfigurationError::PlanStale,
                WriteGateError::Closed => AgentConfigurationError::WriteGateClosed,
                other => AgentConfigurationError::Store(other.to_string()),
            })?;
        let _write_guard = self.acquire_write_guard(&plan.write_context)?;
        if !plan.blocking_activation_skill_ids.is_empty() {
            return Err(AgentConfigurationError::TargetInUse {
                skill_ids: plan.blocking_activation_skill_ids,
            });
        }

        let current_snapshot = self
            .store
            .agent_configuration_snapshot()
            .map_err(map_store_error)?;
        if current_snapshot.snapshot_version != plan.snapshot_version {
            return Err(AgentConfigurationError::PlanStale);
        }
        self.revalidate_name(&current_snapshot, &plan.change)?;
        self.revalidate_home_overlap(&plan.roots)?;
        for root in &plan.roots {
            let current = self
                .filesystem
                .inspect_root(&root.write.configured_path)
                .map_err(map_filesystem_error)?;
            if current != root.inspection {
                return Err(AgentConfigurationError::PlanStale);
            }
        }

        let target = plan.roots.iter().find(|root| {
            root.write.role == AgentRootRole::ActivationTarget && !root.inspection.exists()
        });
        let mut created: Option<CreatedAgentTargetDirectory> = None;
        if let Some(target) = target {
            created = Some(
                self.filesystem
                    .create_target(&target.inspection)
                    .map_err(map_filesystem_error)?,
            );
        }

        let agent_id = match &plan.change {
            AgentConfigurationStoreChange::Create(write)
            | AgentConfigurationStoreChange::Edit(write) => write.agent_id.clone(),
            AgentConfigurationStoreChange::Delete { agent_id } => agent_id.clone(),
        };
        let result = self
            .store
            .apply_agent_configuration_change(plan.snapshot_version, plan.change.clone());
        let generation = match result {
            Ok(generation) => generation,
            Err(error) => {
                if let Some(created) = &created
                    && let Err(rollback) = self.filesystem.rollback_created_target(created)
                {
                    return Err(AgentConfigurationError::RecoveryRequired(
                        rollback.to_string(),
                    ));
                }
                return Err(map_store_error(error));
            }
        };
        let affected_target_root_ids = {
            let mut targets = std::collections::BTreeSet::new();
            match &plan.change {
                AgentConfigurationStoreChange::Create(write)
                | AgentConfigurationStoreChange::Edit(write) => {
                    for root in &write.roots {
                        if root.role == AgentRootRole::ActivationTarget {
                            targets.insert(root.root_id.clone());
                        }
                    }
                    if let AgentConfigurationStoreChange::Edit(_) = &plan.change
                        && let Some(configuration) = current_snapshot
                            .configurations
                            .iter()
                            .find(|configuration| configuration.agent_id == write.agent_id)
                        && let Some(previous_target) = target_root(configuration)
                    {
                        targets.insert(previous_target.root_id.clone());
                    }
                }
                AgentConfigurationStoreChange::Delete { agent_id } => {
                    if let Some(configuration) = current_snapshot
                        .configurations
                        .iter()
                        .find(|configuration| &configuration.agent_id == agent_id)
                        && let Some(previous_target) = target_root(configuration)
                    {
                        targets.insert(previous_target.root_id.clone());
                    }
                }
            }
            targets.into_iter().collect()
        };
        self.plans
            .lock()
            .map_err(|_| AgentConfigurationError::Store("plan lock is poisoned".into()))?
            .remove(plan_token);
        Ok(AgentConfigurationApplyResult {
            agent_id,
            generation,
            deleted: plan.kind == AgentConfigurationPlanKind::Delete,
            affected_target_root_ids,
        })
    }

    fn plan_upsert(
        &self,
        kind: AgentConfigurationPlanKind,
        agent_id: String,
        draft: AgentConfigurationDraft,
    ) -> Result<AgentConfigurationPlan, AgentConfigurationError> {
        self.require_open_gate()?;
        let write_context = self.capture_write_context()?;
        let snapshot = self
            .store
            .agent_configuration_snapshot()
            .map_err(map_store_error)?;
        let existing = snapshot
            .configurations
            .iter()
            .find(|configuration| configuration.agent_id == agent_id);
        if kind == AgentConfigurationPlanKind::Edit && existing.is_none() {
            return Err(AgentConfigurationError::NotFound { agent_id });
        }

        let name = draft.name.trim().to_owned();
        if !(1..=80).contains(&name.chars().count()) {
            return Err(AgentConfigurationError::InvalidName);
        }
        let name_identity_key = agent_name_identity_key(&name);
        if snapshot.configurations.iter().any(|configuration| {
            configuration.agent_id != agent_id
                && configuration.name_identity_key == name_identity_key
        }) {
            return Err(AgentConfigurationError::NameConflict { name });
        }
        let (origin, preset_key, compatibility) = match draft.preset_key.as_deref() {
            Some(preset_key) => {
                let preset = self.presets.get(preset_key).ok_or_else(|| {
                    AgentConfigurationError::PresetNotFound {
                        preset_key: preset_key.to_owned(),
                    }
                })?;
                (
                    AgentConfigurationOrigin::Preset,
                    Some(preset_key.to_owned()),
                    preset.compatibility,
                )
            }
            None => (
                AgentConfigurationOrigin::Custom,
                None,
                Compatibility::Unknown,
            ),
        };
        let project_skills_dir = validate_project_skills_dir(draft.project_skills_dir)?;
        if draft.roots.is_empty() {
            return Err(AgentConfigurationError::RootRequired);
        }
        if draft
            .roots
            .iter()
            .filter(|root| root.role == AgentRootRole::ActivationTarget)
            .count()
            != 1
        {
            return Err(AgentConfigurationError::ActivationTargetRequired);
        }

        let home = self
            .write_gate
            .bound_home()
            .map_err(|_| AgentConfigurationError::WriteGateClosed)?;
        let home_inspection = self
            .filesystem
            .inspect_root(&home.path)
            .map_err(map_filesystem_error)?;
        let mut planned_roots = Vec::with_capacity(draft.roots.len());
        let mut seen = BTreeMap::<String, PathBuf>::new();
        for root in draft.roots {
            let inspection = self
                .filesystem
                .inspect_root(&root.configured_path)
                .map_err(|_| AgentConfigurationError::InvalidRoot {
                    path: root.configured_path.to_string_lossy().into_owned(),
                })?;
            let normalized = inspection.normalized_path.to_string_lossy().into_owned();
            let identity = configured_path_identity_key(&normalized);
            if let Some(existing_path) =
                seen.insert(identity.clone(), inspection.normalized_path.clone())
            {
                return Err(AgentConfigurationError::DuplicateRoot {
                    path: existing_path.to_string_lossy().into_owned(),
                });
            }
            if paths_overlap(
                &inspection.normalized_path,
                &home_inspection.normalized_path,
            ) {
                return Err(AgentConfigurationError::HomeOverlap { path: normalized });
            }
            if root.role == AgentRootRole::ActivationTarget {
                if !inspection.writable {
                    return Err(AgentConfigurationError::TargetNotWritable { path: normalized });
                }
                if target_path_is_forbidden(&inspection.normalized_path) {
                    return Err(AgentConfigurationError::TargetNotAllowed { path: normalized });
                }
            }
            let root_id = snapshot
                .roots
                .iter()
                .find(|stored| stored.path_identity_key == identity)
                .map(|stored| stored.root_id.clone())
                .unwrap_or(self.new_id()?);
            planned_roots.push(PlannedRoot {
                write: AgentRootWrite {
                    root_id,
                    configured_path: inspection.normalized_path.clone(),
                    path_identity_key: identity,
                    role: root.role,
                },
                inspection,
            });
        }
        for (index, root) in planned_roots.iter().enumerate() {
            for other in planned_roots.iter().skip(index + 1) {
                if paths_overlap(
                    &root.inspection.normalized_path,
                    &other.inspection.normalized_path,
                ) {
                    return Err(AgentConfigurationError::RootOverlap {
                        path: root
                            .inspection
                            .normalized_path
                            .to_string_lossy()
                            .into_owned(),
                        conflicting_path: other
                            .inspection
                            .normalized_path
                            .to_string_lossy()
                            .into_owned(),
                    });
                }
            }
        }
        for planned in &planned_roots {
            for stored in &snapshot.roots {
                if stored.path_identity_key == planned.write.path_identity_key
                    || stored
                        .consumer_agent_ids
                        .iter()
                        .all(|consumer| consumer == &agent_id)
                {
                    continue;
                }
                if paths_overlap(&planned.write.configured_path, &stored.configured_path) {
                    return Err(AgentConfigurationError::RootOverlap {
                        path: planned.write.configured_path.to_string_lossy().into_owned(),
                        conflicting_path: stored.configured_path.to_string_lossy().into_owned(),
                    });
                }
            }
        }

        let old_target = existing.and_then(target_root);
        let new_target = planned_roots
            .iter()
            .find(|root| root.write.role == AgentRootRole::ActivationTarget)
            .expect("exactly one target was validated");
        let changing_target = old_target
            .map(|target| target.root_id != new_target.write.root_id)
            .unwrap_or(false);
        let (blocking, retained_activation_count) = if changing_target {
            target_change_consequences(&snapshot, old_target)
        } else {
            (Vec::new(), 0)
        };
        let now = timestamp();
        let write = AgentConfigurationWrite {
            agent_id: agent_id.clone(),
            origin,
            preset_key,
            name,
            name_identity_key,
            compatibility,
            project_skills_dir,
            roots: planned_roots
                .iter()
                .map(|root| root.write.clone())
                .collect(),
            created_at: existing
                .map(|configuration| configuration.created_at.clone())
                .unwrap_or_else(|| now.clone()),
            updated_at: now,
        };
        let change = match kind {
            AgentConfigurationPlanKind::Create => {
                AgentConfigurationStoreChange::Create(write.clone())
            }
            AgentConfigurationPlanKind::Edit => AgentConfigurationStoreChange::Edit(write.clone()),
            AgentConfigurationPlanKind::Delete => unreachable!("delete has a dedicated planner"),
        };
        let configuration = render_planned_configuration(&write, &snapshot.roots);
        let target_will_be_created = !new_target.inspection.exists();
        let plan_token = self.new_id()?;
        self.plans
            .lock()
            .map_err(|_| AgentConfigurationError::Store("plan lock is poisoned".into()))?
            .insert(
                plan_token.clone(),
                PendingPlan {
                    ticket: PlanTicket {
                        generation: write_context.generation,
                    },
                    write_context: write_context.clone(),
                    snapshot_version: snapshot.snapshot_version,
                    kind,
                    change,
                    roots: planned_roots,
                    blocking_activation_skill_ids: blocking.clone(),
                },
            );
        Ok(AgentConfigurationPlan {
            plan_token,
            kind,
            configuration: Some(configuration),
            target_will_be_created,
            blocking_activation_skill_ids: blocking,
            retained_activation_count,
        })
    }

    fn render_snapshot(
        &self,
        snapshot: AgentConfigurationStoreSnapshot,
    ) -> AgentManagementSnapshot {
        let roots = snapshot
            .roots
            .iter()
            .map(|root| (root.root_id.as_str(), root))
            .collect::<BTreeMap<_, _>>();
        let configurations = snapshot
            .configurations
            .iter()
            .map(|configuration| render_configuration(configuration, &roots))
            .collect();
        AgentManagementSnapshot {
            generation: snapshot.snapshot_version,
            configurations,
            presets: self.presets.presets().to_vec(),
        }
    }

    fn revalidate_name(
        &self,
        snapshot: &AgentConfigurationStoreSnapshot,
        change: &AgentConfigurationStoreChange,
    ) -> Result<(), AgentConfigurationError> {
        let write = match change {
            AgentConfigurationStoreChange::Create(write)
            | AgentConfigurationStoreChange::Edit(write) => write,
            AgentConfigurationStoreChange::Delete { .. } => return Ok(()),
        };
        if snapshot.configurations.iter().any(|configuration| {
            configuration.agent_id != write.agent_id
                && configuration.name_identity_key == write.name_identity_key
        }) {
            return Err(AgentConfigurationError::NameConflict {
                name: write.name.clone(),
            });
        }
        Ok(())
    }

    fn revalidate_home_overlap(
        &self,
        roots: &[PlannedRoot],
    ) -> Result<(), AgentConfigurationError> {
        let home = self
            .write_gate
            .bound_home()
            .map_err(|_| AgentConfigurationError::WriteGateClosed)?;
        let home = self
            .filesystem
            .inspect_root(&home.path)
            .map_err(map_filesystem_error)?;
        for root in roots {
            if paths_overlap(&root.write.configured_path, &home.normalized_path) {
                return Err(AgentConfigurationError::HomeOverlap {
                    path: root.write.configured_path.to_string_lossy().into_owned(),
                });
            }
        }
        Ok(())
    }

    fn capture_write_context(&self) -> Result<HomeWriteContext, AgentConfigurationError> {
        self.write_gate
            .capture_open_context()
            .map_err(|_| AgentConfigurationError::WriteGateClosed)
    }

    fn acquire_write_guard(
        &self,
        context: &HomeWriteContext,
    ) -> Result<ProductWriteGuard<'_>, AgentConfigurationError> {
        self.write_gate
            .acquire_product_write(context)
            .map_err(|error| match error {
                WriteGateError::Stale => AgentConfigurationError::PlanStale,
                WriteGateError::Closed => AgentConfigurationError::WriteGateClosed,
                other => AgentConfigurationError::Store(other.to_string()),
            })
    }

    fn require_open_gate(&self) -> Result<(), AgentConfigurationError> {
        if matches!(self.write_gate.snapshot().state, WriteGateState::Open(_)) {
            Ok(())
        } else {
            Err(AgentConfigurationError::WriteGateClosed)
        }
    }

    fn new_id(&self) -> Result<String, AgentConfigurationError> {
        let mut bytes = [0_u8; 16];
        self.filesystem
            .random_bytes(&mut bytes)
            .map_err(map_filesystem_error)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        ))
    }
}

fn render_configuration(
    configuration: &StoredAgentConfiguration,
    roots: &BTreeMap<&str, &StoredGlobalSkillRoot>,
) -> AgentConfiguration {
    let mut rendered_roots = configuration
        .memberships
        .iter()
        .filter_map(|membership| {
            roots
                .get(membership.root_id.as_str())
                .map(|root| AgentConfigurationRoot {
                    root_id: root.root_id.clone(),
                    configured_path: root.configured_path.clone(),
                    path_identity_key: root.path_identity_key.clone(),
                    role: membership.role,
                    consumer_agent_ids: root.consumer_agent_ids.clone(),
                    activation_skill_ids: root.activation_skill_ids.clone(),
                })
        })
        .collect::<Vec<_>>();
    rendered_roots.sort_by_key(|root| root.role != AgentRootRole::ActivationTarget);
    AgentConfiguration {
        agent_id: configuration.agent_id.clone(),
        origin: configuration.origin,
        preset_key: configuration.preset_key.clone(),
        name: configuration.name.clone(),
        compatibility: configuration.compatibility,
        project_skills_dir: configuration.project_skills_dir.clone(),
        roots: rendered_roots,
    }
}

fn render_planned_configuration(
    write: &AgentConfigurationWrite,
    stored_roots: &[StoredGlobalSkillRoot],
) -> AgentConfiguration {
    let roots = write
        .roots
        .iter()
        .map(|root| {
            let stored = stored_roots
                .iter()
                .find(|stored| stored.root_id == root.root_id);
            AgentConfigurationRoot {
                root_id: root.root_id.clone(),
                configured_path: root.configured_path.clone(),
                path_identity_key: root.path_identity_key.clone(),
                role: root.role,
                consumer_agent_ids: stored
                    .map(|stored| stored.consumer_agent_ids.clone())
                    .unwrap_or_else(|| vec![write.agent_id.clone()]),
                activation_skill_ids: stored
                    .map(|stored| stored.activation_skill_ids.clone())
                    .unwrap_or_default(),
            }
        })
        .collect();
    AgentConfiguration {
        agent_id: write.agent_id.clone(),
        origin: write.origin,
        preset_key: write.preset_key.clone(),
        name: write.name.clone(),
        compatibility: write.compatibility,
        project_skills_dir: write.project_skills_dir.clone(),
        roots,
    }
}

fn target_root(
    configuration: &StoredAgentConfiguration,
) -> Option<&crate::seams::agent_configuration_store::StoredAgentRootMembership> {
    configuration
        .memberships
        .iter()
        .find(|membership| membership.role == AgentRootRole::ActivationTarget)
}

fn target_change_consequences(
    snapshot: &AgentConfigurationStoreSnapshot,
    old_target: Option<&crate::seams::agent_configuration_store::StoredAgentRootMembership>,
) -> (Vec<String>, usize) {
    let Some(old_target) = old_target else {
        return (Vec::new(), 0);
    };
    let Some(root) = snapshot
        .roots
        .iter()
        .find(|root| root.root_id == old_target.root_id)
    else {
        return (Vec::new(), 0);
    };
    if root.consumer_agent_ids.len() == 1 {
        (root.activation_skill_ids.clone(), 0)
    } else {
        (Vec::new(), root.activation_skill_ids.len())
    }
}

fn validate_project_skills_dir(
    value: Option<PathBuf>,
) -> Result<Option<PathBuf>, AgentConfigurationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.as_os_str().is_empty()
        || value.is_absolute()
        || value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.to_str().is_none()
    {
        return Err(AgentConfigurationError::InvalidProjectSkillsDir);
    }
    Ok(Some(value))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn target_path_is_forbidden(path: &Path) -> bool {
    [
        Path::new("/System"),
        Path::new("/Library"),
        Path::new("/Applications"),
    ]
    .iter()
    .any(|root| path.starts_with(root))
        || path.components().any(|component| {
            component.as_os_str().to_str().is_some_and(|component| {
                matches!(
                    component.to_ascii_lowercase().as_str(),
                    "caches" | "cache" | "plugins" | "plugin" | "extensions" | "builtin"
                )
            })
        })
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn map_store_error(error: AgentConfigurationStoreError) -> AgentConfigurationError {
    match error {
        AgentConfigurationStoreError::Stale => AgentConfigurationError::PlanStale,
        AgentConfigurationStoreError::NotFound { agent_id } => {
            AgentConfigurationError::NotFound { agent_id }
        }
        AgentConfigurationStoreError::NameConflict => AgentConfigurationError::NameConflict {
            name: String::new(),
        },
        AgentConfigurationStoreError::TargetInUse { skill_ids } => {
            AgentConfigurationError::TargetInUse { skill_ids }
        }
        AgentConfigurationStoreError::Unavailable(message) => {
            AgentConfigurationError::Store(message)
        }
        AgentConfigurationStoreError::RootConflict
        | AgentConfigurationStoreError::InvalidTargetMembership => {
            AgentConfigurationError::PlanStale
        }
    }
}

fn map_filesystem_error(error: AgentConfigurationFileSystemError) -> AgentConfigurationError {
    match error {
        AgentConfigurationFileSystemError::PlanStale => AgentConfigurationError::PlanStale,
        AgentConfigurationFileSystemError::Rollback(message) => {
            AgentConfigurationError::RecoveryRequired(message)
        }
        other => AgentConfigurationError::FileSystem(other.to_string()),
    }
}
