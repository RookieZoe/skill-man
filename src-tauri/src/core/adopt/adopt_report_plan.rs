//! Adopt report plan (spec §4.6, §8.1–§8.2, ADR-0017): the Adopt Module no
//! longer enumerates Roots and holds no in-memory report. It consumes the
//! terminal Scan Report of the Observation and Scan Module — generation-bound
//! pages plus per-entity evidence — and turns explicit selections into a
//! read-only plan: stable Local Link Include and an explicit Conflict Set
//! winner. Git Repository Source candidates are handoff-only: their
//! generation-bound evidence is handed to the Source modules (ADR-0018
//! Source Tracking Policy + immutable Source Transition, #92); no worktree
//! HEAD, dirty bytes, old lock `skillPath`/hash/ref/anchor ever synthesize a
//! plan.
//!
//! Fail-closed rules implemented here (all no-plan paths return before any
//! token is issued):
//! - `plan_report` accepts only the current Open Home's non-Stale Complete/
//!   Incomplete Report; a running Scan, a cross-startup cache, a
//!   cancelled/superseded Run or an old generation all return `PlanStale`;
//! - the closed action vocabulary is `local_link` | `conflict_winner`; any
//!   entity move, deletion, replacement or external-ownership release
//!   returns the typed `scan_coverage_incomplete` closed reason while the
//!   Report is Incomplete and never issues a plan token;
//! - `entity_ref` is an opaque, generation-bound token; plan and apply re-
//!   verify the canonical path/identity/tree/appearances plus the Report
//!   identity, Home/WriteGate, Catalog and Agent generations, then `PlanStale`;
//! - an already-Managed Skill is never replaced; a Conflict Set has exactly
//!   one explicit winner, the other members stay Untracked.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::{
    AdoptAppearance, AdoptAppearanceEvidence, AdoptAppearanceKind, AdoptError, AdoptPlan,
    AdoptPlanItem, AdoptService, PlanInputItem,
};
use crate::core::domain::AgentId;
use crate::core::scan::classification::{
    OP_CONFLICT_WINNER, OP_LOCAL_LINK, VERDICT_ALREADY_MANAGED, VERDICT_BLOCKED,
    VERDICT_CONFLICT_SET, VERDICT_DEFERRED, VERDICT_EXCLUDED, VERDICT_GIT,
    VERDICT_IDENTITY_CONFLICT, VERDICT_LOCAL,
};
use crate::core::scan::{ReportFreshness, ScanCoordinator, ScanEntityEvidence};
use crate::seams::filesystem::{EvidenceChainHop, EvidenceChainHopKind};
use crate::seams::scan_evidence_store::ScanSourceVerdictRecord;

/// Closed Adopt actions (spec §4.6; §8.1 "viewing is never selecting").
pub const ACTION_LOCAL_LINK: &str = "local_link";
pub const ACTION_CONFLICT_WINNER: &str = "conflict_winner";

/// Typed eligibility closed codes of the report plan surface.
pub const ELIGIBILITY_SCAN_INCOMPLETE: &str = "scan_coverage_incomplete";
pub const ELIGIBILITY_ALREADY_MANAGED: &str = "already_managed";
pub const ELIGIBILITY_GIT_HANDOFF: &str = "git_group_handoff";
pub const ELIGIBILITY_REQUIRES_MOVE: &str = "requires_move";
pub const ELIGIBILITY_CONFLICT_WINNER_REQUIRED: &str = "conflict_winner_required";
pub const ELIGIBILITY_DUPLICATE_WINNER: &str = "duplicate_conflict_winner";
pub const ELIGIBILITY_UNKNOWN: &str = "operation_not_available";

/// Opaque, generation-bound reference to one canonical entity of a Report
/// (spec §4.6): only valid for the exact Report identity it was issued for.
/// The token is encoded for the DTO boundary; the frontend never interprets
/// it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptEntityRef {
    pub report_content_identity: String,
    pub generation: u64,
    pub entity_seq: u64,
}

impl AdoptEntityRef {
    pub fn new(
        report_content_identity: impl Into<String>,
        generation: u64,
        entity_seq: u64,
    ) -> Self {
        Self {
            report_content_identity: report_content_identity.into(),
            generation,
            entity_seq,
        }
    }

    /// Opaque token format: `<content identity>@<generation>@<entity seq>`.
    /// The content identity is hex (its own `:` never collides with the
    /// `@` separators).
    pub fn encode(&self) -> String {
        format!(
            "{}@{}@{}",
            self.report_content_identity, self.generation, self.entity_seq
        )
    }

    pub fn decode(token: &str) -> Option<Self> {
        let mut parts = token.splitn(3, '@');
        let content_identity = parts.next()?.to_owned();
        let generation = parts.next()?.parse::<u64>().ok()?;
        let entity_seq = parts.next()?.parse::<u64>().ok()?;
        if content_identity.is_empty() || entity_seq == 0 {
            return None;
        }
        Some(Self {
            report_content_identity: content_identity,
            generation,
            entity_seq,
        })
    }
}

/// One explicit selection of the report plan request (spec §4.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptReportSelection {
    pub entity_ref: String,
    pub action: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptReportPlanRequest {
    pub report_generation: u64,
    pub selections: Vec<AdoptReportSelection>,
}

/// The Report generation facts a plan binds to (spec §4.6 plan/apply
/// revalidation): the Report identity, the Report generation, the Run that
/// published it and the Catalog generation frozen at plan time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptFrozenReport {
    pub report_content_identity: String,
    pub report_generation: u64,
    pub report_run_id: String,
    pub catalog_snapshot_version: u64,
}

/// Frozen per-item facts (spec §8.1 TOCTOU re-check): the entity identity
/// and tree plus every appearance identity as scanned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptFrozenEvidence {
    pub report_content_identity: String,
    pub entity_seq: u64,
    pub canonical_entity: PathBuf,
    pub entity_device: u64,
    pub entity_inode: u64,
    pub tree_hash: String,
    pub appearance_identities: Vec<AppearanceIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppearanceIdentity {
    pub entry_path: PathBuf,
    pub kind: String,
    pub device: u64,
    pub inode: u64,
}

/// The current Report plan context: the exact identity/generation the
/// selections bind to plus the scan Run state gate.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ReportPlanContext {
    content_identity: String,
    generation: u64,
    run_id: String,
    incomplete: bool,
}

impl AdoptService {
    /// Plan from the terminal Scan Report (spec §4.6): only the current
    /// Open Home's non-Stale Complete/Incomplete Report is accepted. Every
    /// selection is re-verified against the frozen report evidence and the
    /// live filesystem before any plan token is issued.
    pub fn plan_report(&self, request: &AdoptReportPlanRequest) -> Result<AdoptPlan, AdoptError> {
        self.ensure_writes_ready()?;
        let context = self.report_plan_context()?;
        if request.report_generation != context.generation {
            return Err(AdoptError::PlanStale);
        }
        let coordinator = self.scan_coordinator();
        let agents = self.store.list_agents()?;
        let agents_by_id = agents
            .iter()
            .map(|agent| (agent.agent_id.clone(), agent.clone()))
            .collect::<HashMap<AgentId, _>>();
        let mut inputs = Vec::with_capacity(request.selections.len());
        let mut plan_items = Vec::with_capacity(request.selections.len());
        let mut winner_sets: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for selection in &request.selections {
            let entity_ref =
                AdoptEntityRef::decode(&selection.entity_ref).ok_or(AdoptError::PlanStale)?;
            if entity_ref.report_content_identity != context.content_identity
                || entity_ref.generation != context.generation
            {
                return Err(AdoptError::PlanStale);
            }
            let evidence = coordinator
                .entity_evidence(
                    &entity_ref.report_content_identity,
                    entity_ref.generation,
                    entity_ref.entity_seq,
                )
                .map_err(|_| AdoptError::PlanStale)?;
            if evidence.verdict.entity_seq != evidence.entity.entity_seq
                || evidence.entity.entity_seq != entity_ref.entity_seq
            {
                return Err(AdoptError::PlanStale);
            }
            // Typed per-selection eligibility (spec §8.1). A refused
            // selection never issues a plan token.
            let (action, is_conflict_winner) = match selection.action.as_str() {
                ACTION_LOCAL_LINK => (ACTION_LOCAL_LINK, false),
                ACTION_CONFLICT_WINNER => (ACTION_CONFLICT_WINNER, true),
                _ => return Err(AdoptError::Validation("unknown Adopt action".into())),
            };
            self.verify_selection_eligibility(&evidence, action, is_conflict_winner, &context)?;
            if is_conflict_winner {
                let set_seq = evidence.verdict.conflict_set_seq.ok_or_else(|| {
                    AdoptError::Internal("a conflict set member has no set".into())
                })?;
                if !winner_sets.insert(set_seq) {
                    return Err(self.eligibility(
                        ELIGIBILITY_DUPLICATE_WINNER,
                        evidence.entity.entity_seq,
                        Some(format!(
                            "the Conflict Set {set_seq} already has an explicit winner"
                        )),
                    ));
                }
            }
            // Freeze the world read-only: any change since the scan is a
            // stale plan (spec §4.6, §11).
            let frozen = self.freeze_report_entity(&evidence)?;
            let (appearances, appearance_evidence, activations) =
                self.build_appearance_inputs(&evidence, &agents_by_id)?;
            let directory_name = evidence
                .verdict
                .directory_names
                .first()
                .cloned()
                .unwrap_or_else(|| {
                    evidence
                        .entity
                        .canonical_path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            let activation_pairs = activations
                .iter()
                .map(|step| (step.entry_path.clone(), step.target_path.clone()))
                .collect::<Vec<_>>();
            plan_items.push(AdoptPlanItem {
                entity_ref: entity_ref.encode(),
                action: action.to_owned(),
                directory_name: directory_name.clone(),
                canonical_entity: evidence.entity.canonical_path.clone(),
                final_entity_path: evidence.entity.canonical_path.clone(),
                appearances: appearance_evidence,
                activations: activation_pairs,
                applyable: true,
                error: None,
            });
            inputs.push(PlanInputItem {
                directory_name: directory_name.clone(),
                kind: super::AdoptPlanKind::Link,
                canonical_entity: evidence.entity.canonical_path.clone(),
                final_entity_path: evidence.entity.canonical_path.clone(),
                appearances,
                activations,
                frozen,
            });
        }
        let plan_number = self
            .next_plan_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let plan_token = format!("adopt-plan-{plan_number}");
        let frozen_report = AdoptFrozenReport {
            report_content_identity: context.content_identity.clone(),
            report_generation: context.generation,
            report_run_id: context.run_id,
            catalog_snapshot_version: self.store.snapshot_version(),
        };
        let batch = self.build_planned_batch(&plan_token, inputs, &frozen_report)?;
        let _ = batch;
        Ok(AdoptPlan {
            plan_token,
            report_generation: context.generation,
            items: plan_items,
            can_apply: true,
        })
    }

    fn scan_coordinator(&self) -> &Arc<ScanCoordinator> {
        self.scan_coordinator
            .as_ref()
            .expect("plan_report requires a scan coordinator")
    }

    /// The acceptable current-Report context for planning: Open Home with a
    /// non-Stale Complete/Incomplete Report; a running/queued Scan, a
    /// cancelled/superseded Run, a cross-startup cache or a missing report
    /// all fail closed (spec §4.6, ADR-0020 `缓存不冒充现场`).
    fn report_plan_context(&self) -> Result<ReportPlanContext, AdoptError> {
        let coordinator = self
            .scan_coordinator
            .as_ref()
            .ok_or(AdoptError::PlanStale)?;
        let snapshot = coordinator.snapshot();
        if snapshot.current_report.freshness != ReportFreshness::Current {
            return Err(AdoptError::PlanStale);
        }
        let summary = snapshot
            .current_report
            .summary
            .as_ref()
            .ok_or(AdoptError::PlanStale)?;
        if let Some(run) = &snapshot.run {
            if !run.state.is_terminal() || run.run_id != summary.run_id {
                return Err(AdoptError::PlanStale);
            }
        }
        Ok(ReportPlanContext {
            content_identity: summary.content_identity.clone(),
            generation: summary.generation,
            run_id: summary.run_id.clone(),
            incomplete: summary.incomplete,
        })
    }

    /// Re-verify the frozen Report at apply time before the first write:
    /// the exact Report identity, the Run that published it, the Home/
    /// WriteGate (already covered by the gate generation), the Catalog
    /// generation, the Agent generation (freshness recomputation) and every
    /// item's frozen entity/identity/tree/appearances.
    pub(super) fn verify_report_plan(
        &self,
        frozen: &AdoptFrozenReport,
        items: &[super::PlannedAdoptItem],
    ) -> Result<(), AdoptError> {
        let context = self.report_plan_context()?;
        if context.content_identity != frozen.report_content_identity
            || context.generation != frozen.report_generation
            || context.run_id != frozen.report_run_id
        {
            return Err(AdoptError::PlanStale);
        }
        if self.store.snapshot_version() != frozen.catalog_snapshot_version {
            return Err(AdoptError::PlanStale);
        }
        for item in items {
            self.recheck_report_entity(&item.frozen)?;
        }
        Ok(())
    }

    fn verify_selection_eligibility(
        &self,
        evidence: &ScanEntityEvidence,
        action: &str,
        is_conflict_winner: bool,
        context: &ReportPlanContext,
    ) -> Result<(), AdoptError> {
        let verdict = &evidence.verdict;
        let allowed = |operation: &str| {
            verdict
                .operations
                .iter()
                .any(|op| op.operation == operation && op.allowed)
        };
        if is_conflict_winner {
            if verdict.verdict != VERDICT_CONFLICT_SET || !allowed(OP_CONFLICT_WINNER) {
                let code = eligibility_code(verdict, action, context.incomplete);
                return Err(self.eligibility(
                    &code,
                    verdict.entity_seq,
                    Some(verdict_detail(verdict)),
                ));
            }
            return Ok(());
        }
        if verdict.verdict != VERDICT_LOCAL || !allowed(OP_LOCAL_LINK) {
            let code = eligibility_code(verdict, action, context.incomplete);
            return Err(self.eligibility(&code, verdict.entity_seq, Some(verdict_detail(verdict))));
        }
        Ok(())
    }

    fn eligibility(&self, code: &str, entity_seq: u64, detail: Option<String>) -> AdoptError {
        AdoptError::Eligibility {
            code: code.to_owned(),
            entity_seq,
            detail,
        }
    }

    /// Freeze the report entity read-only: the canonical identity and tree
    /// hash plus every appearance chain identity. Any mismatch since the
    /// scan is brittle evidence and must never be planned (spec §4.6).
    fn freeze_report_entity(
        &self,
        evidence: &ScanEntityEvidence,
    ) -> Result<AdoptFrozenEvidence, AdoptError> {
        let entity = &evidence.entity;
        let tree_hash = entity.tree_hash.clone().ok_or_else(|| {
            self.eligibility(
                ELIGIBILITY_UNKNOWN,
                entity.entity_seq,
                Some("the entity has no tree hash evidence".into()),
            )
        })?;
        let fingerprint = self
            .filesystem
            .directory_fingerprint(&entity.canonical_path)
            .map_err(|_| AdoptError::PlanStale)?;
        if fingerprint.device != entity.identity.device
            || fingerprint.inode != entity.identity.inode
        {
            return Err(AdoptError::PlanStale);
        }
        let current_tree = self
            .filesystem
            .tree_hash(&entity.canonical_path)
            .map_err(|_| AdoptError::PlanStale)?;
        if current_tree != tree_hash {
            return Err(AdoptError::PlanStale);
        }
        let mut appearance_identities = Vec::with_capacity(evidence.appearances.len());
        for appearance in &evidence.appearances {
            if appearance.chain_fault.is_some()
                || appearance.final_entity.as_ref() != Some(&entity.canonical_path)
            {
                return Err(AdoptError::PlanStale);
            }
            let kind = appearance.entry_kind.as_str();
            let identity = if kind == "symlink" {
                let chain = self
                    .filesystem
                    .inspect_evidence_chain(&appearance.entry_path)
                    .map_err(|_| AdoptError::PlanStale)?;
                if chain.fault.is_some()
                    || chain.final_entity != Some(entity.canonical_path.clone())
                {
                    return Err(AdoptError::PlanStale);
                }
                (
                    chain.entry_path.clone(),
                    chain.entry_device,
                    chain.entry_inode,
                )
            } else {
                // Real directory appearance: the entry IS the entity; its
                // identity must still match the frozen object identity.
                let fingerprint = self
                    .filesystem
                    .directory_fingerprint(&appearance.entry_path)
                    .map_err(|_| AdoptError::PlanStale)?;
                (
                    appearance.entry_path.clone(),
                    fingerprint.device,
                    fingerprint.inode,
                )
            };
            appearance_identities.push(AppearanceIdentity {
                entry_path: identity.0,
                kind: kind.to_owned(),
                device: identity.1,
                inode: identity.2,
            });
        }
        Ok(AdoptFrozenEvidence {
            report_content_identity: self
                .scan_coordinator()
                .snapshot()
                .current_report
                .summary
                .as_ref()
                .map(|summary| summary.content_identity.clone())
                .unwrap_or_default(),
            entity_seq: entity.entity_seq,
            canonical_entity: entity.canonical_path.clone(),
            entity_device: fingerprint.device,
            entity_inode: fingerprint.inode,
            tree_hash,
            appearance_identities,
        })
    }

    /// Re-check one frozen item against the live filesystem and the current
    /// Report right before an apply writes anything (spec §4.6, §11).
    pub(super) fn recheck_report_entity(
        &self,
        frozen: &AdoptFrozenEvidence,
    ) -> Result<(), AdoptError> {
        let coordinator = self
            .scan_coordinator
            .as_ref()
            .ok_or(AdoptError::PlanStale)?;
        let evidence = coordinator
            .entity_evidence(
                &frozen.report_content_identity,
                // The generation is part of the Report identity; the token
                // carries it, but the verification uses the frozen token
                // only for the identity string.
                self.current_generation(&frozen.report_content_identity)?,
                frozen.entity_seq,
            )
            .map_err(|_| AdoptError::PlanStale)?;
        if evidence.entity.canonical_path != frozen.canonical_entity {
            return Err(AdoptError::PlanStale);
        }
        let fingerprint = self
            .filesystem
            .directory_fingerprint(&frozen.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        if fingerprint.device != frozen.entity_device || fingerprint.inode != frozen.entity_inode {
            return Err(AdoptError::PlanStale);
        }
        let current_tree = self
            .filesystem
            .tree_hash(&frozen.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        if current_tree != frozen.tree_hash {
            return Err(AdoptError::PlanStale);
        }
        for identity in &frozen.appearance_identities {
            if identity.kind == "symlink" {
                let chain = self
                    .filesystem
                    .inspect_evidence_chain(&identity.entry_path)
                    .map_err(|_| AdoptError::PlanStale)?;
                if chain.fault.is_some()
                    || chain.entry_path != identity.entry_path
                    || chain.entry_device != identity.device
                    || chain.entry_inode != identity.inode
                    || chain.final_entity != Some(frozen.canonical_entity.clone())
                {
                    return Err(AdoptError::PlanStale);
                }
            } else {
                let fingerprint = self
                    .filesystem
                    .directory_fingerprint(&identity.entry_path)
                    .map_err(|_| AdoptError::PlanStale)?;
                if fingerprint.device != identity.device || fingerprint.inode != identity.inode {
                    return Err(AdoptError::PlanStale);
                }
            }
        }
        Ok(())
    }

    fn current_generation(&self, content_identity: &str) -> Result<u64, AdoptError> {
        let summary = self
            .scan_coordinator
            .as_ref()
            .ok_or(AdoptError::PlanStale)?
            .snapshot()
            .current_report
            .summary
            .ok_or(AdoptError::PlanStale)?;
        if summary.content_identity != content_identity {
            return Err(AdoptError::PlanStale);
        }
        Ok(summary.generation)
    }

    /// Map report appearances into Adopt inputs: the per-appearance evidence
    /// chain for presentation plus the activation steps for every
    /// keep-in-place symlink appearance whose Root is consumer-agent-backed
    /// (the Agent configuration Activation Target). Real-directory
    /// appearances never get an activation (the entity already sits in the
    /// Root in place).
    fn build_appearance_inputs(
        &self,
        evidence: &ScanEntityEvidence,
        agents_by_id: &HashMap<AgentId, crate::seams::adopt_store::AdoptAgent>,
    ) -> Result<AppearanceInputs, AdoptError> {
        let entity = &evidence.entity;
        let mut appearances = Vec::with_capacity(evidence.appearances.len());
        let mut evidence_rows = Vec::with_capacity(evidence.appearances.len());
        let mut activations = Vec::new();
        for appearance in &evidence.appearances {
            let kind = if appearance.entry_kind == "symlink" {
                let original_target = appearance
                    .chain
                    .first()
                    .and_then(|hop| hop.target.clone())
                    .unwrap_or_else(|| entity.canonical_path.clone());
                AdoptAppearanceKind::Symlink { original_target }
            } else {
                AdoptAppearanceKind::RealDirectory
            };
            let chain = appearance
                .chain
                .iter()
                .map(map_chain_hop)
                .collect::<Vec<_>>();
            appearances.push(AdoptAppearance {
                entry_path: appearance.entry_path.clone(),
                kind: kind.clone(),
            });
            evidence_rows.push(AdoptAppearanceEvidence {
                entry_path: appearance.entry_path.clone(),
                kind: kind.clone(),
                chain,
            });
            if appearance.entry_kind == "symlink" {
                let root = evidence
                    .roots
                    .iter()
                    .find(|root| root.index == appearance.root_index);
                if let Some(root) = root {
                    for consumer in &root.consumer_agents {
                        if let Some(agent) = agents_by_id
                            .get(&crate::core::domain::AgentId(consumer.agent_id.clone()))
                        {
                            if agent.activation_target {
                                activations.push(crate::seams::filesystem::AdoptActivationStep {
                                    target_root_id: agent.root_id.clone(),
                                    entry_path: appearance.entry_path.clone(),
                                    target_path: entity.canonical_path.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok((appearances, evidence_rows, activations))
    }
}

type AppearanceInputs = (
    Vec<AdoptAppearance>,
    Vec<AdoptAppearanceEvidence>,
    Vec<crate::seams::filesystem::AdoptActivationStep>,
);

fn map_chain_hop(hop: &crate::seams::scan_evidence_store::ScanChainHopRecord) -> EvidenceChainHop {
    let kind = if hop.kind == "symlink" {
        EvidenceChainHopKind::Symlink {
            target: hop.target.clone().unwrap_or_default(),
        }
    } else {
        EvidenceChainHopKind::Directory
    };
    EvidenceChainHop {
        path: hop.path.clone(),
        kind,
        device: hop.device,
        inode: hop.inode,
    }
}

/// Closed typed eligibility code for a selection the planner refuses
/// (spec §8.2 vocabulary; the presentation degrades on the code, never on
/// raw detail).
fn eligibility_code(verdict: &ScanSourceVerdictRecord, action: &str, incomplete: bool) -> String {
    match verdict.verdict.as_str() {
        VERDICT_GIT => verdict
            .reason_kind
            .clone()
            .unwrap_or_else(|| ELIGIBILITY_GIT_HANDOFF.to_owned()),
        VERDICT_ALREADY_MANAGED => ELIGIBILITY_ALREADY_MANAGED.to_owned(),
        VERDICT_CONFLICT_SET => {
            if action == ACTION_LOCAL_LINK {
                ELIGIBILITY_CONFLICT_WINNER_REQUIRED.to_owned()
            } else {
                ELIGIBILITY_UNKNOWN.to_owned()
            }
        }
        VERDICT_IDENTITY_CONFLICT => verdict
            .reason_kind
            .clone()
            .unwrap_or_else(|| "identity_conflict".to_owned()),
        VERDICT_BLOCKED => verdict
            .reason_kind
            .clone()
            .unwrap_or_else(|| "blocked".to_owned()),
        VERDICT_DEFERRED => verdict
            .reason_kind
            .clone()
            .unwrap_or_else(|| "deferred".to_owned()),
        VERDICT_EXCLUDED => "excluded".to_owned(),
        VERDICT_LOCAL => {
            if incomplete {
                ELIGIBILITY_SCAN_INCOMPLETE.to_owned()
            } else {
                ELIGIBILITY_REQUIRES_MOVE.to_owned()
            }
        }
        _ => ELIGIBILITY_UNKNOWN.to_owned(),
    }
}

fn verdict_detail(verdict: &ScanSourceVerdictRecord) -> String {
    let mut detail = String::new();
    if let Some(reason) = &verdict.reason_kind {
        detail.push_str(reason);
    }
    if let Some(facts) = &verdict.detail {
        if !detail.is_empty() {
            detail.push_str(": ");
        }
        detail.push_str(facts);
    }
    detail
}
