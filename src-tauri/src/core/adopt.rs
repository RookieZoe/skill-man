//! Adopt module (ADR-0005): scan Untracked Skills in Agent and shared
//! directories, plan per-Skill transactions, apply them with a durable
//! journal, and undo a whole batch before the result window closes.
//!
//! Every candidate is one Skill with one canonical final entity. Real
//! directories migrate into the Library (file Install); symlinks whose final
//! entity lies outside every Agent directory register as Links. Shared
//! entries (`~/.agents/skills`) are split into per-Agent Activations. Each
//! Skill is its own transaction: a failure rolls back only that Skill.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;

use crate::core::domain::{AgentId, AgentKind, SkillId, parse_skill_metadata, skill_identity_key};
use crate::core::import::LibraryConflict;
use crate::seams::adopt_store::{
    AdoptAgent, AdoptStore, AdoptStoreError, AdoptedActivation, AdoptedSkillRecord,
};
use crate::seams::clock::Clock;
use crate::seams::filesystem::{
    ActivationEntrySnapshot, AdoptActivationStep, AdoptAppearanceKind as JournalAppearanceKind,
    AdoptAppearanceStep, AdoptItemPhase, AdoptJournal, AdoptJournalItem, AdoptJournalKind,
    AdoptJournalPhase, DirectoryFingerprint, FileSystem, FileSystemError, LinkSourceEntryKind,
    StagedTreeSnapshot,
};
use crate::seams::recovery::RecoveryGate;

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
pub enum AdoptRisk {
    None,
    /// The entity lives outside the home directory or in an installer-managed
    /// location; adoptable only with explicit confirmation.
    External,
    /// The entry is dangling: no final entity exists, so it cannot be Adopted.
    Broken,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AdoptScanReport {
    pub candidates: Vec<AdoptCandidate>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptCandidate {
    /// The canonical final entity; for Broken candidates the entry path.
    pub canonical_entity: PathBuf,
    /// Product identity: the lexicographically first directory name.
    pub directory_name: String,
    pub directory_names: Vec<String>,
    pub appearances: Vec<AdoptAppearance>,
    pub risk: AdoptRisk,
    pub risk_reason: Option<String>,
    /// A Managed Skill already uses this identity with a different entity.
    pub conflict: Option<LibraryConflict>,
    pub adoptable: bool,
    /// For shared candidates: detected Agents that read the shared directory.
    pub suggested_agent_ids: Vec<AgentId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptPlanKind {
    /// The entity is moved into the Library (file Install).
    Migrate,
    /// The entity stays outside; the Library records a pointer (Link).
    Link,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptSelection {
    pub canonical_entity: PathBuf,
    /// Overrides the suggested Agents for shared candidates.
    pub agent_ids: Vec<AgentId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlanItem {
    pub directory_name: String,
    pub canonical_entity: PathBuf,
    pub kind: AdoptPlanKind,
    pub final_entity_path: PathBuf,
    pub appearances: Vec<AdoptAppearance>,
    pub target_agents: Vec<AdoptAgent>,
    pub adoptable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlan {
    pub plan_token: String,
    pub items: Vec<AdoptPlanItem>,
    pub can_apply: bool,
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
    tree_snapshot: Option<StagedTreeSnapshot>,
    appearances: Vec<AdoptAppearance>,
    activations: Vec<AdoptActivationStep>,
    journal: AdoptJournalItem,
}

#[derive(Clone)]
struct PlannedAdoptBatch {
    operation_id: String,
    items: Vec<PlannedAdoptItem>,
    journal: AdoptJournal,
    created_at_millis: u128,
}

pub struct AdoptService {
    store: Arc<dyn AdoptStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    library_root: PathBuf,
    home_directory: PathBuf,
    plans: Mutex<HashMap<String, PlannedAdoptBatch>>,
    applied: Mutex<HashMap<String, PlannedAdoptBatch>>,
    next_plan_id: AtomicU64,
    next_skill_id: AtomicU64,
    plan_ttl: Duration,
    recovery_gate: Arc<RecoveryGate>,
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
            library_root,
            home_directory,
            plans: Mutex::new(HashMap::new()),
            applied: Mutex::new(HashMap::new()),
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

    /// Scan every detected Agent skills directory plus the shared/legacy
    /// source (`~/.agents/skills`), grouping entries by canonical final
    /// entity. Library-owned entries, dot-prefixed entries and Codex's
    /// `.system` are excluded; entries without a readable SKILL.md are not
    /// candidates. The report is truncated at `MAX_ADOPT_SKILLS` with the
    /// truncation flagged (spec §6.6), never silently.
    pub fn scan(&self) -> Result<AdoptScanReport, AdoptError> {
        let agents = self.store.list_agents()?;
        // The canonical home (e.g. /private/var on macOS) is the boundary for
        // external-risk classification; canonical entities always live under it.
        let canonical_home = self
            .home_directory
            .canonicalize()
            .unwrap_or_else(|_| self.home_directory.clone());
        let mut grouped: BTreeMap<PathBuf, GroupedCandidate> = BTreeMap::new();
        for agent in &agents {
            if !agent.detected {
                continue;
            }
            for entry in self.filesystem.scan_skills_directory(&agent.skills_path)? {
                self.accumulate(entry, Some(agent), false, &mut grouped)?;
            }
        }
        let shared_dir = self.home_directory.join(".agents").join("skills");
        for entry in self.filesystem.scan_skills_directory(&shared_dir)? {
            self.accumulate(entry, None, true, &mut grouped)?;
        }
        let suggested = agents
            .iter()
            .filter(|agent| {
                agent.detected
                    && matches!(agent.kind, AgentKind::ClaudePreset | AgentKind::CodexPreset)
            })
            .map(|agent| agent.agent_id.clone())
            .collect::<Vec<_>>();
        let mut candidates = Vec::new();
        for (entity, group) in grouped {
            if !group.broken_entries.is_empty() {
                let entry = &group.broken_entries[0];
                candidates.push(AdoptCandidate {
                    canonical_entity: entity,
                    directory_name: entry.name.clone(),
                    directory_names: vec![entry.name.clone()],
                    appearances: Vec::new(),
                    risk: AdoptRisk::Broken,
                    risk_reason: Some("the entry is dangling: its target no longer exists".into()),
                    conflict: None,
                    adoptable: false,
                    suggested_agent_ids: Vec::new(),
                });
                continue;
            }
            let directory_names = group.directory_names.iter().cloned().collect::<Vec<_>>();
            let directory_name = directory_names[0].clone();
            let conflict = self
                .store
                .find_library_conflict(&skill_identity_key(&directory_name))?
                .filter(|conflict| conflict.final_entity_path != entity)
                .map(|conflict| LibraryConflict {
                    existing_skill_id: conflict.skill_id,
                    directory_name: conflict.directory_name,
                });
            let (risk, risk_reason) = classify_risk(&entity, &canonical_home);
            let shared = group.appearances.iter().any(|appearance| appearance.shared);
            candidates.push(AdoptCandidate {
                canonical_entity: entity,
                directory_name,
                directory_names,
                appearances: group.appearances,
                risk,
                risk_reason,
                conflict: conflict.clone(),
                adoptable: conflict.is_none() && risk != AdoptRisk::Broken,
                suggested_agent_ids: if shared {
                    suggested.clone()
                } else {
                    Vec::new()
                },
            });
        }
        candidates.sort_by(|left, right| left.directory_name.cmp(&right.directory_name));
        let truncated = candidates.len() > MAX_ADOPT_SKILLS;
        candidates.truncate(MAX_ADOPT_SKILLS);
        Ok(AdoptScanReport {
            candidates,
            truncated,
        })
    }

    fn accumulate(
        &self,
        entry: crate::seams::filesystem::ScannedSkillEntry,
        agent: Option<&AdoptAgent>,
        shared: bool,
        grouped: &mut BTreeMap<PathBuf, GroupedCandidate>,
    ) -> Result<(), AdoptError> {
        if entry.name.starts_with('.') {
            return Ok(());
        }
        if let Some(agent) = agent {
            if agent.kind == AgentKind::CodexPreset && entry.name == ".system" {
                return Ok(());
            }
        }
        let Some(final_entity) = entry.final_entity_path.clone() else {
            grouped
                .entry(entry.entry_path.clone())
                .or_default()
                .broken_entries
                .push(entry);
            return Ok(());
        };
        if final_entity.starts_with(&self.library_root) {
            return Ok(());
        }
        if !self.filesystem.skill_directory_is_readable(&final_entity)? {
            return Ok(());
        }
        let group = grouped.entry(final_entity).or_default();
        group.directory_names.insert(entry.name.clone());
        group.appearances.push(AdoptAppearance {
            entry_path: entry.entry_path,
            kind: match entry.kind {
                LinkSourceEntryKind::Directory => AdoptAppearanceKind::RealDirectory,
                LinkSourceEntryKind::Symlink { target } => AdoptAppearanceKind::Symlink {
                    original_target: target,
                },
            },
            agent_id: agent.map(|agent| agent.agent_id.clone()),
            shared,
        });
        Ok(())
    }

    /// Preview an Adopt batch: re-scan (the plan is stale if a candidate
    /// disappeared), stage migrating entities, and persist a durable journal
    /// before any confirmation.
    pub fn plan(&self, selections: &[AdoptSelection]) -> Result<AdoptPlan, AdoptError> {
        self.ensure_writes_ready()?;
        let agents = self.store.list_agents()?;
        let candidates = self.scan()?.candidates;
        let mut planned = Vec::with_capacity(selections.len());
        let mut can_apply = true;
        for selection in selections {
            let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.canonical_entity == selection.canonical_entity)
            else {
                planned.push(AdoptPlanItem {
                    directory_name: String::new(),
                    canonical_entity: selection.canonical_entity.clone(),
                    kind: AdoptPlanKind::Migrate,
                    final_entity_path: PathBuf::new(),
                    appearances: Vec::new(),
                    target_agents: Vec::new(),
                    adoptable: false,
                    error: Some("the candidate disappeared since the scan; rescan first".into()),
                });
                can_apply = false;
                continue;
            };
            if !candidate.adoptable {
                planned.push(AdoptPlanItem {
                    directory_name: candidate.directory_name.clone(),
                    canonical_entity: candidate.canonical_entity.clone(),
                    kind: AdoptPlanKind::Migrate,
                    final_entity_path: PathBuf::new(),
                    appearances: candidate.appearances.clone(),
                    target_agents: Vec::new(),
                    adoptable: false,
                    error: Some(if candidate.risk == AdoptRisk::Broken {
                        "Broken entries cannot be Adopted; repair or remove them first".into()
                    } else if let Some(conflict) = &candidate.conflict {
                        format!(
                            "Managed Skill '{}' already uses this directory identity",
                            conflict.directory_name
                        )
                    } else {
                        "this candidate is not adoptable".into()
                    }),
                });
                can_apply = false;
                continue;
            }
            let inside_agent_dir = agents.iter().any(|agent| {
                agent.detected && candidate.canonical_entity.starts_with(&agent.skills_path)
            });
            let has_real_directory = candidate
                .appearances
                .iter()
                .any(|appearance| appearance.kind == AdoptAppearanceKind::RealDirectory);
            let kind = if has_real_directory || inside_agent_dir {
                AdoptPlanKind::Migrate
            } else {
                AdoptPlanKind::Link
            };
            let final_entity_path = if matches!(kind, AdoptPlanKind::Migrate) {
                self.library_root
                    .join("skills")
                    .join(&candidate.directory_name)
            } else {
                candidate.canonical_entity.clone()
            };
            let target_agents = if candidate
                .appearances
                .iter()
                .any(|appearance| appearance.shared)
            {
                let ids = if selection.agent_ids.is_empty() {
                    candidate.suggested_agent_ids.clone()
                } else {
                    selection.agent_ids.clone()
                };
                agents
                    .iter()
                    .filter(|agent| ids.contains(&agent.agent_id))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            planned.push(AdoptPlanItem {
                directory_name: candidate.directory_name.clone(),
                canonical_entity: candidate.canonical_entity.clone(),
                kind,
                final_entity_path,
                appearances: candidate.appearances.clone(),
                target_agents,
                adoptable: true,
                error: None,
            });
        }
        if !can_apply {
            return Ok(AdoptPlan {
                plan_token: String::new(),
                items: planned,
                can_apply: false,
            });
        }
        self.build_planned_batch(planned, &agents)
    }

    fn build_planned_batch(
        &self,
        items: Vec<AdoptPlanItem>,
        agents: &[AdoptAgent],
    ) -> Result<AdoptPlan, AdoptError> {
        let operation_id = format!(
            "adopt-{}-{}",
            self.clock.unix_epoch_nanos(),
            self.next_plan_id.fetch_add(1, Ordering::Relaxed)
        );
        let staging_operation_root = self.library_root.join("staging").join(&operation_id);
        std::fs::create_dir_all(&staging_operation_root).map_err(|source| {
            AdoptError::FileSystem(FileSystemError::Io {
                operation: "create Adopt staging directory",
                path: staging_operation_root.clone(),
                source,
            })
        })?;
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
            let mut tree_snapshot = None;
            let mut staged_fingerprint = DirectoryFingerprint {
                canonical_path: PathBuf::new(),
                device: 0,
                inode: 0,
            };
            let mut staged_root = PathBuf::new();
            let skill_markdown = if matches!(item.kind, AdoptPlanKind::Migrate) {
                staged_root = staging_operation_root.join(&item.directory_name);
                staged_fingerprint = self
                    .filesystem
                    .stage_external_directory(&item.canonical_entity, &staged_root)?;
                let snapshot = self.filesystem.staged_tree_snapshot(&staged_root)?;
                crate::core::import::validate_staged_tree(self.filesystem.as_ref(), &snapshot)
                    .map_err(adopt_validation)?;
                let markdown =
                    self.filesystem
                        .read_skill_document(&staged_root)
                        .map_err(|error| {
                            AdoptError::Validation(format!("SKILL.md is not readable: {error}"))
                        })?;
                tree_snapshot = Some(snapshot);
                markdown
            } else {
                self.filesystem
                    .read_skill_document(&item.canonical_entity)
                    .map_err(|error| {
                        AdoptError::Validation(format!("SKILL.md is not readable: {error}"))
                    })?
            };
            let metadata = parse_skill_metadata(&skill_markdown);
            let identity_key = crate::core::import::normalize_identity(&item.directory_name)
                .map_err(adopt_validation)?;
            let required_space = tree_snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .total_file_bytes
                        .saturating_mul(2)
                        .saturating_add(DISK_SPACE_RESERVE_BYTES)
                })
                .unwrap_or(0);
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
            let recorded_content_hash = tree_snapshot
                .as_ref()
                .map(|snapshot| snapshot.content_hash.clone())
                .unwrap_or_default();
            let journal = AdoptJournalItem {
                skill_id: skill_id.0.clone(),
                directory_name: item.directory_name.clone(),
                kind: if matches!(item.kind, AdoptPlanKind::Migrate) {
                    AdoptJournalKind::Migrate
                } else {
                    AdoptJournalKind::Link
                },
                staged_root: staged_root.clone(),
                staged_fingerprint: staged_fingerprint.clone(),
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
                phase: AdoptItemPhase::Staged,
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
                tree_snapshot,
                appearances: item.appearances.clone(),
                activations,
                journal,
            });
        }
        let staging_fingerprint = self
            .filesystem
            .directory_fingerprint(&staging_operation_root)?;
        let journal = AdoptJournal {
            version: 1,
            operation_id: operation_id.clone(),
            phase: AdoptJournalPhase::Planned,
            staging_operation_root,
            staging_fingerprint,
            items: planned_items
                .iter()
                .map(|item| item.journal.clone())
                .collect(),
        };
        self.filesystem
            .write_adopt_journal(&self.library_root, &journal)?;
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("adopt-plan-{plan_number}");
        let plan_items = planned_items
            .iter()
            .map(|item| AdoptPlanItem {
                directory_name: item.directory_name.clone(),
                canonical_entity: item.canonical_entity.clone(),
                kind: item.kind,
                final_entity_path: item.final_entity_path.clone(),
                appearances: item.appearances.clone(),
                target_agents: agents
                    .iter()
                    .filter(|agent| {
                        item.activations
                            .iter()
                            .any(|activation| activation.agent_id == agent.agent_id.0)
                    })
                    .cloned()
                    .collect(),
                adoptable: true,
                error: None,
            })
            .collect();
        let batch = PlannedAdoptBatch {
            operation_id,
            items: planned_items,
            journal,
            created_at_millis: self.clock.monotonic_millis(),
        };
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?;
        plans.insert(plan_token.clone(), batch);
        Ok(AdoptPlan {
            plan_token,
            items: plan_items,
            can_apply: true,
        })
    }

    /// Discard a planned (not yet applied) batch: remove its staging content
    /// and archive the journal.
    pub fn cancel(&self, plan_token: &str) -> Result<bool, AdoptError> {
        let batch = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?
            .remove(plan_token);
        let Some(batch) = batch else {
            return Ok(false);
        };
        self.filesystem.discard_staging(
            &batch.journal.staging_operation_root,
            &self.library_root,
            Some(&batch.journal.staging_fingerprint),
        )?;
        self.filesystem
            .finish_adopt_journal(&self.library_root, &batch.operation_id)?;
        Ok(true)
    }

    /// Apply the planned batch. Each Skill is its own transaction with its
    /// own rollback; failures leave the other Skills (and their Activations)
    /// intact.
    pub fn apply(&self, plan_token: &str) -> Result<AdoptResult, AdoptError> {
        self.ensure_writes_ready()?;
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?;
        let batch = plans.remove(plan_token).ok_or(AdoptError::PlanNotFound)?;
        drop(plans);
        if self
            .clock
            .monotonic_millis()
            .saturating_sub(batch.created_at_millis)
            >= self.plan_ttl.as_millis()
        {
            return Err(AdoptError::PlanNotFound);
        }
        let mut journal = batch.journal.clone();
        journal.phase = AdoptJournalPhase::Applying;
        self.filesystem
            .write_adopt_journal(&self.library_root, &journal)?;
        let mut results = Vec::with_capacity(batch.items.len());
        let mut snapshot_version = 0_u64;
        for (index, item) in batch.items.iter().enumerate() {
            match self.apply_item(&mut journal, index, item) {
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
                    results.push(AdoptSkillResult {
                        skill_id: item.skill_id.clone(),
                        directory_name: item.directory_name.clone(),
                        adopted: false,
                        error: Some(match rollback {
                            Ok(()) => error.to_string(),
                            Err(compensation) => {
                                format!("{error}; rollback also failed: {compensation}")
                            }
                        }),
                    });
                }
            }
        }
        self.filesystem
            .discard_staging(
                &journal.staging_operation_root,
                &self.library_root,
                Some(&journal.staging_fingerprint),
            )
            .map_err(|error| {
                AdoptError::RecoveryRequired(format!(
                    "Adopt staging cleanup failed after apply: {error}"
                ))
            })?;
        journal.phase = AdoptJournalPhase::Committed;
        self.filesystem
            .write_adopt_journal(&self.library_root, &journal)?;
        let undo_available = results.iter().any(|result| result.adopted);
        let operation_id = batch.operation_id.clone();
        if undo_available {
            self.applied
                .lock()
                .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
                .insert(batch.operation_id.clone(), batch);
        } else {
            self.filesystem
                .finish_adopt_journal(&self.library_root, &operation_id)?;
        }
        Ok(AdoptResult {
            operation_id,
            items: results,
            snapshot_version,
            undo_available,
        })
    }

    fn apply_item(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<u64, AdoptError> {
        if let Some(snapshot) = &item.tree_snapshot {
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
                .tree_snapshot
                .as_ref()
                .map(|_| item.final_entity_path.clone()),
            final_entity_path: item.final_entity_path.clone(),
            recorded_content_hash: item
                .tree_snapshot
                .as_ref()
                .map(|snapshot| snapshot.content_hash.clone()),
            original_path: item
                .tree_snapshot
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
        Ok(snapshot_version)
    }

    fn rollback_item(
        &self,
        journal: &mut AdoptJournal,
        index: usize,
        item: &PlannedAdoptItem,
    ) -> Result<(), AdoptError> {
        let entry = &mut journal.items[index];
        if entry.phase == AdoptItemPhase::Done || entry.phase == AdoptItemPhase::Staged {
            return Ok(());
        }
        for activation in item.activations.iter().rev() {
            if matches!(
                self.filesystem.activation_snapshot(&activation.entry_path)?,
                ActivationEntrySnapshot::Symlink { target }
                    if target == activation.target_path
            ) {
                self.filesystem.remove_activation(&activation.entry_path)?;
            }
        }
        self.restore_entity_and_appearances(item).map_err(|error| {
            AdoptError::RecoveryRequired(format!(
                "the entity could not be restored to its original location: {error}"
            ))
        })?;
        if entry.phase == AdoptItemPhase::CatalogCommitted
            || entry.phase == AdoptItemPhase::AppearancesApplied
        {
            self.store.remove_adopted_skill(&item.skill_id)?;
        }
        entry.installed_fingerprint = None;
        entry.phase = AdoptItemPhase::Staged;
        self.filesystem
            .write_adopt_journal(&self.library_root, journal)?;
        Ok(())
    }

    /// Move a migrated entity back to its real-directory appearance and
    /// recreate every symlink appearance (the entries were freed by removing
    /// the Activations first).
    fn restore_entity_and_appearances(&self, item: &PlannedAdoptItem) -> Result<(), AdoptError> {
        for appearance in item.appearances.iter().rev() {
            match &appearance.kind {
                AdoptAppearanceKind::RealDirectory => {
                    let source = if item.tree_snapshot.is_some() {
                        &item.final_entity_path
                    } else {
                        &item.staged_root
                    };
                    let current = self.filesystem.directory_fingerprint(source)?;
                    self.filesystem.restore_external_directory(
                        source,
                        &appearance.entry_path,
                        &current,
                    )?;
                }
                AdoptAppearanceKind::Symlink { original_target } => {
                    if !appearance.entry_path.exists() {
                        std::os::unix::fs::symlink(original_target, &appearance.entry_path)
                            .map_err(|source| FileSystemError::Io {
                                operation: "restore Adopt appearance symlink",
                                path: appearance.entry_path.clone(),
                                source,
                            })?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Undo a whole applied batch while its result window is open. Each Skill
    /// is restored in reverse order; an item whose original location is now
    /// occupied is skipped and reported, never overwritten.
    pub fn undo(&self, operation_id: &str) -> Result<AdoptUndoResult, AdoptError> {
        let batch = self
            .applied
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
            .remove(operation_id)
            .ok_or(AdoptError::PlanNotFound)?;
        let mut results = Vec::with_capacity(batch.items.len());
        let mut snapshot_version = 0_u64;
        for item in batch.items.iter().rev() {
            match self.undo_item(item) {
                Ok(version) => {
                    snapshot_version = version;
                    results.push(AdoptUndoItemResult {
                        directory_name: item.directory_name.clone(),
                        undone: true,
                        error: None,
                    });
                }
                Err(error) => results.push(AdoptUndoItemResult {
                    directory_name: item.directory_name.clone(),
                    undone: false,
                    error: Some(error.to_string()),
                }),
            }
        }
        self.filesystem
            .finish_adopt_journal(&self.library_root, operation_id)?;
        Ok(AdoptUndoResult {
            operation_id: operation_id.to_owned(),
            items: results,
            snapshot_version,
        })
    }

    fn undo_item(&self, item: &PlannedAdoptItem) -> Result<u64, AdoptError> {
        for activation in item.activations.iter().rev() {
            if matches!(
                self.filesystem.activation_snapshot(&activation.entry_path)?,
                ActivationEntrySnapshot::Symlink { target }
                    if target == activation.target_path
            ) {
                self.filesystem.remove_activation(&activation.entry_path)?;
            }
        }
        self.restore_entity_and_appearances(item)?;
        let version = self.store.remove_adopted_skill(&item.skill_id)?;
        Ok(version)
    }

    /// Close the result window: archive the journal and drop the batch, so
    /// the batch can no longer be undone.
    pub fn finalize(&self, operation_id: &str) -> Result<(), AdoptError> {
        self.applied
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt applied lock poisoned".into()))?
            .remove(operation_id);
        self.filesystem
            .finish_adopt_journal(&self.library_root, operation_id)?;
        Ok(())
    }

    fn ensure_writes_ready(&self) -> Result<(), AdoptError> {
        if self.recovery_gate.writes_are_ready() {
            Ok(())
        } else {
            Err(AdoptError::RecoveryRequired(
                "startup recovery is still in progress".into(),
            ))
        }
    }
}

#[derive(Default)]
struct GroupedCandidate {
    directory_names: BTreeSet<String>,
    appearances: Vec<AdoptAppearance>,
    broken_entries: Vec<crate::seams::filesystem::ScannedSkillEntry>,
}

fn classify_risk(entity: &Path, home_directory: &Path) -> (AdoptRisk, Option<String>) {
    if !entity.starts_with(home_directory) {
        return (
            AdoptRisk::External,
            Some(format!(
                "the final entity lies outside the home directory: {}",
                entity.display()
            )),
        );
    }
    const EXTERNAL_MARKERS: [&str; 4] = ["Caches", "node_modules", ".Trash", "Downloads"];
    for component in entity.components() {
        if let Component::Normal(value) = component {
            if EXTERNAL_MARKERS.iter().any(|marker| value == *marker) {
                return (
                    AdoptRisk::External,
                    Some(format!(
                        "the final entity lives in an installer-managed location: {}",
                        entity.display()
                    )),
                );
            }
        }
    }
    (AdoptRisk::None, None)
}

fn adopt_validation(error: crate::core::import::ImportError) -> AdoptError {
    AdoptError::Validation(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::classify_risk;
    use std::path::Path;

    #[test]
    fn risk_classifies_external_and_managed_locations() {
        let home = Path::new("/Users/test");
        assert_eq!(
            classify_risk(Path::new("/Users/test/.claude/skills/foo"), home).0,
            super::AdoptRisk::None
        );
        assert_eq!(
            classify_risk(Path::new("/Users/other/.claude/skills/foo"), home).0,
            super::AdoptRisk::External
        );
        assert_eq!(
            classify_risk(Path::new("/Users/test/Library/Caches/installer/foo"), home).0,
            super::AdoptRisk::External
        );
        assert_eq!(
            classify_risk(Path::new("/Users/test/projects/node_modules/foo"), home).0,
            super::AdoptRisk::External
        );
        assert_eq!(
            classify_risk(Path::new("/Users/test/projects/foo"), home).0,
            super::AdoptRisk::None
        );
    }
}
