import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type CatalogFilter =
  | "all"
  | "broken"
  | "modified"
  | "link"
  | "install"
  | "local"
  | "git"
  | "enabled"
  | "disabled";
export type SourceKind = "link" | "remote_install" | "file_install";
export type GitSourceCapabilityKind =
  | "git_repository_source"
  | "legacy_per_skill_git_state"
  | "remote_source_identity_conflict";

export interface GitSourceCapabilitySource {
  remoteId: string;
  canonicalUrl: string;
  kind: GitSourceCapabilityKind;
  /** Complete only for `git_repository_source`; every other kind is empty. */
  members: GitSourceCapabilityMember[];
  provider?: string;
  trackingMode?: string;
  trackingValue?: string | null;
  selectedRef?: string;
  resolvedCommit?: string;
}

export interface GitSourceCapabilityMember {
  pluginName?: string | null;
  skillId: string;
  skillPath: string;
  /** `false` marks a tombstoned member (no bytes to copy). */
  presence: boolean;
}

export interface GitSourceCapabilityReport {
  sources: GitSourceCapabilitySource[];
}
export type Health =
  "healthy" | "broken" | "modified" | "source_snapshot_mismatch";
export type AgentKind = "claude_preset" | "codex_preset" | "custom";
export type Compatibility = "verified" | "unknown";

export type CatalogAccess = "read_write" | "read_only";
export type CatalogReadOnlyReason =
  "unsupported_schema" | "integrity_failed" | "open_failed";

/** Raw technical facts from the native authority, never App Copy (spec §4.7). */
export type HomeCandidateMode = "fresh" | "legacy_in_place" | "legacy_copy";

/** A validated, not-yet-confirmed Home candidate (spec §4.2). */
export interface HomeCandidate {
  path: string;
  token: string;
  mode: HomeCandidateMode;
  volumeFsid: string;
  volumeUuid: string;
  availableBytes: number;
  legacySource: string | null;
}

/** Immutable, read-only evidence for an Existing Home Recovery Plan. */
export type RecoveryProfileFact =
  | "marker_catalog_identity"
  | "standard_layout"
  | "catalog_integrity"
  | "catalog_foreign_keys"
  | "catalog_capabilities";

/** A prepared Existing Home Recovery Plan; confirmation uses this opaque token. */
export interface ExistingHomeRecoveryPlan {
  path: string;
  homeId: string;
  createdAt: string;
  planToken: string;
  facts: RecoveryProfileFact[];
}

export interface BootstrapDiagnostic {
  code: string;
  message: string;
}

/** Code-only reason that keeps an unsafe default Home route closed. */
export type DefaultHomeRecoveryBlockedReason =
  | "not_directory"
  | "marker_missing_or_invalid"
  | "layout_capabilities"
  | "catalog_missing"
  | "catalog_unreadable"
  | "catalog_identity_missing"
  | "home_identity_mismatch"
  | "creation_time_mismatch"
  | "catalog_integrity"
  | "catalog_foreign_keys"
  | "catalog_capabilities"
  | "active_writer"
  | "operation_recovery_required"
  | "fixture_contamination"
  | "unreadable"
  | "recovery_ineligible";

/**
 * The closed top-level bootstrap route union (spec §4.2). React renders
 * exactly one route per `state`; no variant is composed from booleans.
 */
export type BootstrapSnapshot =
  | { state: "app_state_unavailable"; diagnostic: BootstrapDiagnostic | null }
  | { state: "unconfigured" }
  | { state: "default_home_recovery_offer"; path: string }
  | {
      state: "default_home_recovery_blocked";
      path: string;
      reason: DefaultHomeRecoveryBlockedReason;
    }
  | {
      state: "abandoned";
      homeId: string;
      path: string;
    }
  | { state: "legacy_detected"; path: string }
  | {
      state: "fixture_recovery_locked";
      homeId: string | null;
      path: string | null;
    }
  | { state: "home_candidate_pending"; path: string; operationId: string }
  | {
      state: "bound";
      homeId: string;
      catalogAccess: CatalogAccess;
      catalogReadonlyReason: CatalogReadOnlyReason | null;
      snapshotVersion: number;
    }
  | {
      state: "home_unavailable";
      homeId: string;
      path: string;
      diagnostic: BootstrapDiagnostic | null;
    }
  | {
      state: "home_identity_mismatch";
      homeId: string;
      path: string;
      diagnostic: BootstrapDiagnostic | null;
    };

/** The high-friction Abandon preview (ADR-0012 §6). */
export interface AbandonPreview {
  homeId: string;
  path: string;
  boundAt: string;
  planToken: string;
}

/** Why Restore applies (closed reasons; presentation maps to message keys). */
export type RestoreReason =
  "catalog_integrity_failed" | "fixture_contamination";

export type RestoreNotApplicableReason =
  | "no_binding"
  | "legacy_unbound"
  | "app_state_unavailable"
  | "identity_mismatch"
  | "home_unavailable"
  | "unsupported_schema"
  | "open_failed"
  | "active_operation";

/** Restore eligibility probe result (spec §5.5). */
export type RestoreEligibility =
  | {
      kind: "restore_required";
      homeId: string;
      path: string;
      reason: RestoreReason;
    }
  | { kind: "not_required" }
  | { kind: "not_applicable"; reason: RestoreNotApplicableReason };

/** `bootstrap://changed` payload: snapshot plus the write-gate generation. */
export interface BootstrapChangedPayload {
  snapshot: BootstrapSnapshot;
  generation: number;
}

// -- Locale Authority (spec §4.5, §6.1) --

/** The persisted App-level choice; `system` negotiates on activation. */
export type LocaleSelection = "system" | "en" | "zh-Hans";

/** The resolved locale every visible surface renders in. */
export type EffectiveLocale = "en" | "zh-Hans";

/**
 * `locale://changed` payload and the query snapshot are isomorphic (spec
 * §4.7): selection, the effective locale, and a generation that increments
 * on every published change.
 */
export interface LocaleSnapshot {
  selection: LocaleSelection;
  effectiveLocale: EffectiveLocale;
  generation: number;
  /** Raw fallback diagnostic (corrupt/unknown persisted value), never copy. */
  diagnostic: BootstrapDiagnostic | null;
}

// -- Fixture Recovery (spec §4.4, §5.2) --

export type RecoveryMode =
  { kind: "legacy_unbound" } | { kind: "bound_restore"; homeId: string };

export type FixtureClassification =
  | { kind: "pure" }
  | { kind: "mixed"; reasons: string[] }
  | { kind: "unknown"; reasons: string[] }
  | { kind: "clean" };

export interface CatalogEvidence {
  tables: string[];
  schemaVersion: number | null;
  firstRunCompletedAt: string | null;
  skillRowCount: number;
  agentRowCount: number;
  activationRowCount: number;
  fileSourceRowCount: number;
  remoteSourceRowCount: number;
}

export interface TreeEvidence {
  fixtureEntitiesPresent: boolean;
  skillAuthoringHashMatches: boolean | null;
  mediaXrayHashMatches: boolean | null;
  rootHashMatches: boolean | null;
  legacyAuditEntityPresent: boolean;
}

export interface ActiveRecoveryOperation {
  operationId: string;
  cursor: string | null;
  snapshotPath: string | null;
  preparedPath: string | null;
}

export interface FixtureRecoveryPreview {
  mode: RecoveryMode;
  path: string;
  classification: FixtureClassification;
  catalogEvidence: CatalogEvidence;
  treeEvidence: TreeEvidence;
  canPreview: boolean;
  activeOperation: ActiveRecoveryOperation | null;
}

export interface RecoveryResult {
  operationId: string;
  awaitingCommit: boolean;
  rolledBack: boolean;
}

export interface SafetySnapshot {
  snapshotId: string;
  path: string;
  manifestHash: string | null;
  fileCount: number;
  totalBytes: number;
  takenAt: string | null;
}

export interface DeleteSnapshotPreview {
  snapshotId: string;
  path: string;
  fileCount: number;
  totalBytes: number;
}

export interface CommandFailure {
  error: PublicError;
  diagnostic: { code: string; message: string } | null;
}

/** Closed candidate-validation reasons emitted by the native Home Binding API. */
export type CandidateInvalidReason =
  | "not_absolute"
  | "not_utf8"
  | "symlink_component"
  | "state_dir_overlap"
  | "agent_dir_overlap"
  | "parent_missing"
  | "parent_not_writable"
  | "not_directory"
  | "not_empty"
  | "no_volume_identity"
  | "insufficient_space"
  | "not_legacy_home"
  | "legacy_contaminated";

/** Closed reasons that make an Existing Home Recovery preview ineligible. */
export type ExistingHomeRecoveryEligibilityRejection =
  | "current_binding"
  | "abandoned_history"
  | "active_recovery_ledger"
  | "bootstrap_state";

/** Closed reasons an existing Home fails the Recovery Profile. */
export type ExistingHomeRecoveryProfileRejection =
  | "not_directory"
  | "marker_missing_or_invalid"
  | "layout_capabilities"
  | "catalog_missing"
  | "catalog_unreadable"
  | "catalog_identity_missing"
  | "home_identity_mismatch"
  | "creation_time_mismatch"
  | "catalog_integrity"
  | "catalog_foreign_keys"
  | "catalog_capabilities"
  | "active_writer"
  | "operation_recovery_required"
  | "fixture_contamination";

/** The closed public error union (spec §4.7): presentation maps `code` to a
 * message key; typed fields carry Source Content only. */
export type PublicError =
  | { code: "validation" }
  | { code: "not_found" }
  | { code: "agent_configuration_name_invalid" }
  | { code: "agent_configuration_name_conflict"; name: string }
  | { code: "agent_preset_not_found"; presetKey: string }
  | { code: "agent_root_required" }
  | { code: "agent_activation_target_required" }
  | { code: "agent_root_duplicate"; path: string }
  | { code: "agent_root_overlap"; path: string; conflictingPath: string }
  | { code: "agent_root_home_overlap"; path: string }
  | { code: "agent_root_invalid"; path: string }
  | { code: "agent_target_not_writable"; path: string }
  | { code: "agent_target_not_allowed"; path: string }
  | { code: "agent_project_skills_dir_invalid" }
  | { code: "agent_configuration_not_found"; agentId: string }
  | { code: "agent_target_in_use"; skillIds: string[] }
  | { code: "conflict"; directoryName: string }
  | { code: "plan_stale" }
  | { code: "permission_denied" }
  | { code: "state_unavailable" }
  | { code: "catalog_unavailable" }
  | { code: "recovery_required" }
  | { code: "source_unavailable" }
  | { code: "target_mismatch" }
  | { code: "disk_full"; requiredBytes: number; availableBytes: number }
  | { code: "modified" }
  | { code: "stale_update" }
  | { code: "update_cancelled" }
  | { code: "download_failed" }
  | { code: "install_failed" }
  | { code: "bootstrap_unavailable" }
  | { code: "recovery_not_locked" }
  | { code: "recovery_not_pure" }
  | { code: "recovery_no_active_operation" }
  | { code: "recovery_operation_already_active" }
  | { code: "recovery_writer_active" }
  | { code: "recovery_step_failed" }
  | { code: "recovery_state_ambiguous" }
  | { code: "recovery_snapshot_in_use" }
  | { code: "recovery_state_store" }
  | { code: "recovery_filesystem" }
  | { code: "recovery_probe" }
  | { code: "restore_not_applicable" }
  | { code: "reconnect_not_available" }
  | { code: "abandon_not_applicable" }
  | { code: "abandon_confirmation_mismatch" }
  | { code: "abandon_cas_conflict" }
  | { code: "candidate_invalid"; reason: CandidateInvalidReason }
  | { code: "binding_step_failed"; cursor: string }
  | { code: "binding_state_ambiguous" }
  | { code: "binding_not_cancellable" }
  | { code: "binding_migration_failed" }
  | {
      code: "existing_home_recovery_ineligible";
      reason: ExistingHomeRecoveryEligibilityRejection;
    }
  | {
      code: "existing_home_recovery_profile_rejected";
      reason: ExistingHomeRecoveryProfileRejection;
    }
  | { code: "locale_store_unavailable" }
  | { code: "scan_not_writable" }
  | { code: "scan_run_not_found" }
  | { code: "scan_report_not_found" }
  | { code: "scan_report_stale"; currentGeneration: number }
  | {
      code: "adopt_eligibility";
      closedCode: string;
      entitySeq: number;
      detail: string | null;
    }
  | { code: "internal" };

export interface SkillSummary {
  id: string;
  directoryName: string;
  displayName: string;
  description: string;
  sourceKind: SourceKind;
  health: Health;
  enabledAgentCount: number;
}

export interface CatalogList {
  snapshotVersion: number;
  items: SkillSummary[];
}

export interface SkillDetail extends SkillSummary {
  finalEntityPath: string;
  /** Raw Source Content: the original file Install path (never App Copy). */
  fileSourceOriginalPath: string | null;
  frontmatterName: string | null;
  lastActivityAt: string;
  skillMarkdown: string;
}

// -- Global Enable (spec §4.9; ADR-0019) --

/** Typed Target availability: a missing/unverifiable Target only routes
 * to Agent Management and is never created by Enable. */
export type TargetGroupAvailability = "available" | "absent" | "unavailable";

/** The only typed action a Target group exposes when it cannot support
 * Enable: `open_agent_management`. */
export type TargetGroupAction = "none" | "open_agent_management";

export interface TargetGroupMember {
  agentId: string;
  agentName: string;
  compatibility: Compatibility;
  userConfigured?: boolean;
}

export interface GlobalTargetGroup {
  /** Canonical Target identity (shared root id after path dedup). */
  targetRootId: string;
  configuredPath: string;
  /** Every Agent Configuration pointing its Activation Target here. */
  consumers: TargetGroupMember[];
  availability: TargetGroupAvailability;
  /** Raw probe failure reason, never user copy. */
  diagnostic: string | null;
  /** The current Skill's desired state in this group. */
  desired: boolean;
  /** Last Activation health observation for this `(Skill, Target)`. */
  observedState:
    "present" | "missing" | "target_mismatch" | "dangling" | "occupied" | null;
  action: TargetGroupAction;
}

export interface GlobalTargetGroupSnapshot {
  skillId: string;
  skillName: string;
  /** Current Skill health from the Catalog authority. */
  skillHealth: Health;
  agentGeneration: number;
  groups: GlobalTargetGroup[];
}

export type EnableAction = "enable" | "disable" | "repair" | "switch";
export type CellResolution = "switch" | "replace" | "adopt" | "skip";
export type CellEligibility =
  "ready" | "no_op" | "skipped" | "conflict" | "blocked";
export type CellBlockedReason =
  | "project_source_choice"
  | "project_source_not_selected"
  | "invalid_project_copy"
  | "invalid_copy_payload"
  | "non_portable_project_alias"
  | "target_absent"
  | "target_unavailable"
  | "source_snapshot_mismatch"
  | "tombstoned_member"
  | "entity_broken"
  | "entry_occupied"
  | "outside_project_root"
  | "symlink_cycle"
  | "hop_limit_exceeded"
  | "target_not_directory";

export interface ProjectRootEvidence {
  canonicalPath: string;
  identity: string;
  hopEvidence?: ProjectHopEvidence[];
}

export interface ProjectHopEvidence {
  agentId: string;
  agentName: string;
  configuredRelativePath: string;
  resolvedContainer: string;
  hops: EvidenceChainHop[];
}

export interface RecentProjectFolder {
  canonicalPathKey: string;
  canonicalPath: string;
  lastUsedAt: string;
}

export interface DestructiveCounts {
  directories: number;
  files: number;
}

export interface EnableCell {
  projectCopy?: "create" | "reuse" | null;
  dependsOnCopy?: string | null;
  /** `"<skill_id>|<target_root_id>"` cell identity. */
  cellKey: string;
  skillId: string;
  skillName: string;
  directoryName: string;
  directoryIdentityKey: string;
  targetRootId: string;
  targetPath: string;
  entryPath: string;
  finalEntityPath: string;
  action: EnableAction;
  affectedAgentIds: string[];
  affectedAgentNames: string[];
  occupier:
    | "empty"
    | { managed: { skillId: string; directoryName: string } }
    | {
        untracked: {
          kind: "symlink" | "real_directory" | "file";
          target: string | null;
        };
      };
  occExactDirect: boolean;
  destructive: DestructiveCounts | null;
  eligibility: CellEligibility;
  blockedReason: CellBlockedReason | null;
  resolution: CellResolution;
  detail: string | null;
  createSteps: string[];
  hopEvidence: ProjectHopEvidence[];
}

export interface EnablePlan {
  planToken: string;
  scope: string;
  writeGateGeneration: number;
  catalogGeneration: number;
  agentGeneration: number;
  projectRoot?: ProjectRootEvidence | null;
  cells: EnableCell[];
}

export type CellOutcome =
  "succeeded" | "no_op" | "skipped" | "failed" | "not_attempted";

export interface EnableCellResult {
  projectCopy?: "create" | "reuse" | null;
  dependsOnCopy?: string | null;
  copyReady?: boolean | null;
  cellKey: string;
  skillId: string;
  targetRootId: string;
  outcome: CellOutcome;
  diagnostic: string | null;
}

export interface EnableResult {
  operationId: string;
  cells: EnableCellResult[];
  snapshotVersion: number;
}

export interface EnableUndoCellResult {
  cellKey: string;
  undone: boolean;
  diagnostic: string | null;
}

export interface EnableUndoResult {
  recoveryRequired?: boolean;
  operationId: string;
  cells: EnableUndoCellResult[];
  snapshotVersion: number;
}

export interface CellResolutionRequest {
  cellKey: string;
  resolution: CellResolution;
}

export type AgentConfigurationOrigin = "preset" | "custom";
export type AgentRootRole = "scan_only" | "activation_target";

export interface AgentConfigurationRoot {
  rootId: string;
  configuredPath: string;
  pathIdentityKey: string;
  role: AgentRootRole;
  consumerAgentIds: string[];
  activationSkillIds: string[];
}

export interface AgentConfiguration {
  agentId: string;
  origin: AgentConfigurationOrigin;
  presetKey: string | null;
  name: string;
  compatibility: Compatibility;
  projectSkillsDir: string | null;
  roots: AgentConfigurationRoot[];
}

export interface AgentPreset {
  presetKey: string;
  name: string;
  compatibility: Compatibility;
  roots: string[];
  activationTarget: string;
  projectSkillsDir: string;
}

export interface AgentManagementSnapshot {
  generation: number;
  configurations: AgentConfiguration[];
  presets: AgentPreset[];
}

// -- Observation and Scan Module (spec §4.10; ADR-0020) --

/** Closed per-root Detection state; probe failures are `unavailable` with a
 * diagnostic and are never downgraded to `absent`. */
export type RootDetectionState = "present" | "unavailable" | "absent";

export interface RootObservation {
  configuredPath: string;
  state: RootDetectionState;
  /** Resolved directory identity when `present`. */
  canonicalPath: string | null;
  /** Raw reason when `unavailable`; never user copy. */
  diagnostic: string | null;
}

/** Closed per-preset Detection state; `unknown` is the honest pre-run state. */
export type PresetDetectionState =
  "present" | "unavailable" | "absent" | "unknown";

export interface PresetObservation {
  presetKey: string;
  name: string;
  state: PresetDetectionState;
  roots: RootObservation[];
}

/** In-memory Detection result: bounded to the nine fixed Presets. */
export interface DetectionSnapshot {
  generation: number;
  presetObservations: PresetObservation[];
  /** The run crossed the 1-second threshold (spec §4.10). */
  slow: boolean;
}

/** Observation lifecycle; `stale` is the cross-startup/kept-old view. */
export type ObservationStatus = "unknown" | "checking" | "observed" | "stale";

export interface StartupProbeRootCounts {
  total: number;
  present: number;
  unavailable: number;
  absent: number;
}

export interface StartupProbeTargetCounts {
  total: number;
  present: number;
  unavailable: number;
  absent: number;
}

/** Bounded Startup Probe summary (spec §4.10 `startupProbe`). */
export interface StartupProbeSnapshot {
  generation: number;
  status: ObservationStatus;
  rootCounts: StartupProbeRootCounts;
  targetCounts: StartupProbeTargetCounts;
  slow: boolean;
  diagnostic: string | null;
}

export interface ActivationHealthCounts {
  targetGroups: number;
  observedGroups: number;
  failedGroups: number;
  unresponsiveGroups: number;
  entriesTotal: number;
  entriesPresent: number;
  entriesUnhealthy: number;
  entriesUnknown: number;
}

/** Bounded Activation Health summary (spec §4.10 `activationHealth`). */
export interface ActivationHealthSnapshot {
  generation: number;
  status: ObservationStatus;
  targetGroupCounts: ActivationHealthCounts;
  slow: boolean;
  diagnostic: string | null;
}

export type ObservationKind = "startup_probe" | "activation_health";

/** Stable cursor into one observation generation (spec §4.10). */
export interface ObservationCursor {
  offset: number;
}

/** One Startup Probe row (read-only Root/Target observation). */
export interface StartupProbeRow {
  configuredPath: string;
  pathIdentityKey: string;
  canonicalPath: string | null;
  state: "present" | "unavailable" | "absent";
  diagnostic: string | null;
  isTarget: boolean;
}

export type ActivationObservedState =
  "present" | "missing" | "target_mismatch" | "dangling" | "occupied";

/** One Activation health row (an enabled `(Skill, Target)` activation). */
export interface ActivationHealthRow {
  skillId: string;
  targetRootId: string;
  entryPath: string;
  /** This generation's observation; `null` = Unknown (kept old). */
  observedState: ActivationObservedState | null;
  /** The previously persisted observation (kept old on failure). */
  previousState: ActivationObservedState | null;
  stale: boolean;
  diagnostic: string | null;
  checkedAtMs: number | null;
}

export type ObservationRow =
  | { kind: "startup_probe"; row: StartupProbeRow }
  | { kind: "activation_health"; row: ActivationHealthRow };

/** A bounded page of one observation generation (spec §4.10). */
export interface ObservationPage {
  generation: number;
  rows: ObservationRow[];
  nextOffset: number | null;
}

/**
 * The `observation://changed` payload and the query snapshot are isomorphic
 * (spec §4.10): the same bounded summary with the same generations.
 */
export interface ObservationAndScanSnapshot {
  homeId: string | null;
  writeGateGeneration: number;
  agentConfigurationGeneration: number | null;
  detection: DetectionSnapshot;
  /** Startup Probe bounded summary (spec §4.10). */
  startupProbe: StartupProbeSnapshot | null;
  /** Activation Health bounded summary (spec §4.10). */
  activationHealth: ActivationHealthSnapshot | null;
  /** The single-flight Rescan Run (spec §4.10 `scanRun`). */
  scanRun: ScanRunSnapshot | null;
  /** Bounded current Report view (`currentReport`). */
  currentReport: CurrentReport;
}

/** Closed Run lifecycle states (spec §4.10). */
export type ScanRunState =
  | "queued"
  | "running"
  | "cancelling"
  | "cancelled"
  | "superseded"
  | "completed"
  | "failed";

export type ScanPhase = "planning" | "walking" | "hashing" | "finalizing";

export interface ScanCounts {
  roots: number;
  entries: number;
  entities: number;
  files: number;
  bytes: number;
  gitProbes: number;
  failedRoots: number;
  /** Funnel: configured Agent Configurations contributing roots. */
  configuredAgents: number;
  /** Funnel: configured Root declarations before the canonical union. */
  declaredRoots: number;
  /** Funnel: distinct physical Roots after the canonical union. */
  canonicalRoots: number;
}

/** Real phase/Root/count/elapsed facts — never a percent or ETA. */
export interface ScanRunSnapshot {
  runId: string;
  generation: number;
  trigger: "onboarding" | "manual";
  state: ScanRunState;
  phase: ScanPhase;
  currentRoot: number | null;
  counts: ScanCounts;
  roots: ScanRootView[];
  elapsedMs: number;
  slow: boolean;
  diagnostic: string | null;
}

export interface ScanRootView {
  index: number;
  configuredPath: string;
  canonicalPath: string;
  state: "pending" | "walking" | "completed" | "failed" | "unresponsive";
  counts: ScanCounts;
  elapsedMs: number;
  slow: boolean;
  diagnostic: string | null;
}

/** Layered Root coverage of a terminal Report (spec §4.6 `coverageCounts`). */
export interface ScanCoverageCounts {
  completed: number;
  failed: number;
  unresponsive: number;
}

export interface ScanReportSummary {
  generation: number;
  runId: string;
  /** The unforgeable Report identity page cursors bind to. */
  contentIdentity: string;
  trigger: "onboarding" | "manual";
  state: "complete" | "incomplete";
  coverage: ScanCoverageCounts;
  counts: ScanCounts;
  incomplete: boolean;
  publishedAtMs: number;
  agentConfigurationGeneration: number;
  configuredRootSnapshotFingerprint: string;
  startedAtMs: number;
  slow: boolean;
  /** Bounded source classification counts (spec §8.1). */
  sourceCounts: ScanSourceCounts;
}

/** Bounded source classification counts of a terminal Report (spec §8.1). */
export interface ScanSourceCounts {
  gitGroups: number;
  gitGroupsConflicted: number;
  localCandidates: number;
  conflictSets: number;
  conflictMembers: number;
  blocked: number;
  deferred: number;
  identityConflicts: number;
  alreadyManaged: number;
  excluded: number;
  needsAttention: number;
}

/** Generation-bound object identity (ADR-0017): Report-generation scoped. */
export interface ScanObjectIdentity {
  device: number;
  inode: number;
}

export interface ScanChainHop {
  path: string;
  kind: string;
  device: number;
  inode: number;
  target: string | null;
}

export interface ScanChainFault {
  kind: string;
  at: string;
  detail: string | null;
}

export interface ScanLockHint {
  lockPath: string;
  entryName: string;
  fingerprint: string;
  faulted: boolean;
  fault: string | null;
  /** Declaring strict-parse entry facts (never the lock body). */
  sourceType: string | null;
  sourceUrl: string | null;
  requestedRef: string | null;
  skillPath: string | null;
}

/** One enriched applicable lock claim (External Ownership Claim, §8.1). */
export interface ScanLockClaim {
  lockPath: string;
  entryName: string;
  fingerprint: string;
  sourceType: string | null;
  sourceUrl: string | null;
  requestedRef: string | null;
  skillPath: string | null;
}

/** Typed operation eligibility of one candidate (spec §8.1). */
export interface ScanOperationEligibility {
  operation: string;
  allowed: boolean;
  closedReason: string | null;
}

/** One Root consumer Agent (spec §7.6 coverage table). */
export interface ScanRootAgent {
  agentId: string;
  agentName: string;
}

export interface ScanWorktreeHint {
  repositoryRoot: string;
  gitdirKind: string;
  remoteUrls: string[];
  headRef: string | null;
}

/** One row of a Report page; the section determines the shape. */
export type ScanReportRow =
  | {
      kind: "root_coverage";
      index: number;
      configuredPath: string;
      canonicalPath: string;
      state: "completed" | "failed" | "unresponsive";
      consumerAgents: ScanRootAgent[];
      counts: ScanCounts;
      elapsedMs: number;
      slow: boolean;
      diagnostic: string | null;
    }
  | {
      kind: "entity";
      entitySeq: number;
      identity: ScanObjectIdentity;
      canonicalPath: string;
      fileCount: number;
      byteCount: number;
      treeHash: string | null;
      hashFault: string | null;
      appearances: number;
      firstRootIndex: number;
      firstEntrySeq: number;
    }
  | {
      kind: "appearance";
      rootIndex: number;
      seq: number;
      name: string;
      entryPath: string;
      entryKind: "directory" | "symlink";
      chain: ScanChainHop[];
      chainFault: ScanChainFault | null;
      finalEntity: string | null;
      identity: ScanObjectIdentity | null;
      entitySeq: number | null;
      lockHint: ScanLockHint | null;
      worktreeHint: ScanWorktreeHint | null;
    }
  | {
      kind: "git_source_group";
      groupSeq: number;
      provider: string;
      canonicalRepository: string;
      repositoryRoot: string | null;
      remoteUrlsSeen: string[];
      memberEntitySeqs: number[];
      memberPaths: string[];
      memberNames: string[];
      lockClaims: ScanLockClaim[];
      refs: string[];
      lockPaths: string[];
      status: "candidate" | "repository_ref_conflict" | "ownership_split";
      operations: ScanOperationEligibility[];
      detail: string | null;
    }
  | {
      kind: "source_verdict";
      /** Opaque generation-bound entity reference (spec §4.6): the
       * presentation passes it back verbatim, never interprets it. */
      entityRef: string;
      entitySeq: number;
      verdict:
        | "local"
        | "git"
        | "conflict_set"
        | "identity_conflict"
        | "blocked"
        | "deferred"
        | "already_managed"
        | "excluded";
      canonicalPath: string;
      directoryNames: string[];
      appearances: number;
      fileCount: number;
      byteCount: number;
      treeHash: string | null;
      lockClaims: ScanLockClaim[];
      worktreeHints: ScanWorktreeHint[];
      reasonKind: string | null;
      detail: string | null;
      gitRefs: string[];
      gitLockPaths: string[];
      gitGroupSeq: number | null;
      conflictSetSeq: number | null;
      notes: string[];
      operations: ScanOperationEligibility[];
    }
  | {
      kind: "conflict_set";
      setSeq: number;
      directoryIdentityKey: string;
      directoryName: string;
      memberEntitySeqs: number[];
      /** Opaque generation-bound entity references, index-aligned with
       * `memberEntitySeqs` (spec §4.6). */
      memberEntityRefs: string[];
      memberPaths: string[];
      winnerEntitySeq: number | null;
    }
  | {
      kind: "diagnostic";
      rootIndex: number;
      diagnosticKind: string;
      at: string | null;
      detail: string | null;
    };

export type ScanReportSection =
  | "roots"
  | "entities"
  | "appearances"
  | "diagnostics"
  | "git_sources"
  | "local_candidates"
  | "conflict_sets"
  | "needs_attention"
  | "excluded";

/** A stable Report cursor (spec §4.10): pinned to one Report identity. */
export interface ScanReportCursor {
  reportContentIdentity: string;
  runId: string;
  generation: number;
  section: ScanReportSection;
  offset: number;
}

/** A bounded page of the current Report (spec §4.10). */
export interface ScanReportPage {
  reportContentIdentity: string;
  runId: string;
  generation: number;
  section: ScanReportSection;
  rows: ScanReportRow[];
  nextOffset: number | null;
}

export type ReportFreshness = "current" | "stale";

export type StaleReason =
  | "cross_startup"
  | "configuration_changed"
  | "home_or_gate_changed"
  | "filesystem_changed"
  | "cache_unreadable";

export interface CurrentReport {
  summary: ScanReportSummary | null;
  freshness: ReportFreshness;
  staleReasons: StaleReason[];
}

export interface AgentRootDraft {
  configuredPath: string;
  role: AgentRootRole;
}

export interface AgentConfigurationDraft {
  presetKey: string | null;
  name: string;
  roots: AgentRootDraft[];
  projectSkillsDir: string | null;
}

export interface AgentConfigurationPlan {
  planToken: string;
  kind: "create" | "edit" | "delete";
  configuration: AgentConfiguration | null;
  targetWillBeCreated: boolean;
  blockingActivationSkillIds: string[];
  retainedActivationCount: number;
}

export interface AgentConfigurationApplyResult {
  agentId: string;
  generation: number;
  deleted: boolean;
  /** Target Root ids affected by the apply (Target-scoped health). */
  affectedTargetRootIds: string[];
}

export interface AppPreferences {
  launchAtLogin: boolean;
  showInDock: boolean;
  checkAppUpdates: boolean;
  checkSkillUpdates: boolean;
}

export interface PreferenceUpdates {
  launchAtLogin?: boolean;
  showInDock?: boolean;
  checkAppUpdates?: boolean;
  checkSkillUpdates?: boolean;
}

export interface UpdatePreferencesResult {
  preferences: AppPreferences;
  warning: PreferencesWarning | null;
}

export type PreferencesWarning =
  | { kind: "show_in_dock_failed"; detail: string }
  | { kind: "launch_at_login_failed"; detail: string };

export type AppUpdateCheck =
  | { status: "skipped" }
  | { status: "up_to_date" }
  | {
      status: "available";
      version: string;
      currentVersion: string;
      releaseNotes: string;
      downloadSizeBytes: number;
      updateId: string;
    };

export type AvailableAppUpdate = Extract<
  AppUpdateCheck,
  { status: "available" }
>;

export interface DownloadedAppUpdate {
  updateId: string;
  version: string;
}

export interface CancelledAppUpdate {
  updateId: string;
}

export interface StartupAgent {
  id: string;
  name: string;
  kind: AgentKind;
  skillsPath: string;
  detected: boolean;
}

export interface StartupInfo {
  firstRun: boolean;
  /** The bootstrap-verified Home path, present in the product runtime. */
  libraryPath?: string | null;
  agents: StartupAgent[];
}

export interface RelocateLinkPreview {
  planToken: string;
  skillId: string;
  directoryName: string;
  sourceEntryPath: string;
  finalEntityPath: string;
  displayName: string;
  description: string;
  frontmatterName: string | null;
  activationCount: number;
}

export interface RelocateLinkResult {
  skillId: string;
  directoryName: string;
  finalEntityPath: string;
  activationCount: number;
  snapshotVersion: number;
}

export interface RemoveSkillPreview {
  planToken: string;
  skillId: string;
  directoryName: string;
  sourceKind: SourceKind;
  finalEntityPath: string;
  activationCount: number;
}

export interface RemoveSkillResult {
  skillId: string;
  directoryName: string;
  snapshotVersion: number;
}

export interface LinkImportCandidate {
  directoryName: string;
  displayName: string;
  description: string;
  frontmatterName: string | null;
  sourceEntryPath: string;
  finalEntityPath: string;
}

export interface LibraryConflict {
  existingSkillId: string;
  directoryName: string;
}

export interface LinkImportPreview {
  planToken: string;
  directoryName: string;
  displayName: string;
  description: string;
  sourceEntryPath: string;
  finalEntityPath: string;
  libraryEntryPath: string | null;
  conflict: LibraryConflict | null;
  canApply: boolean;
}

export interface LinkImportResult {
  operationId: string;
  skillId: string;
  directoryName: string;
  finalEntityPath: string;
  libraryEntryPath: string | null;
  snapshotVersion: number;
}

export interface GitImportCandidate {
  directoryName: string;
  displayName: string;
  description: string;
  frontmatterName: string | null;
  skillPath: string;
}

export interface GitImportDiscovery {
  repoUrl: string;
  requestedRef: string;
  resolvedCommit: string;
  candidates: GitImportCandidate[];
  truncated: boolean;
}

export interface GitImportPreview {
  planToken: string;
  directoryName: string;
  displayName: string;
  description: string;
  skillPath: string;
  finalEntityPath: string;
  conflict: LibraryConflict | null;
  canApply: boolean;
}

export interface GitImportSelectionPreview {
  planToken: string;
  repoUrl: string;
  requestedRef: string;
  resolvedCommit: string;
  items: GitImportPreview[];
  canApply: boolean;
}

export interface GitImportResult {
  operationId: string;
  skillId: string;
  directoryName: string;
  finalEntityPath: string;
  snapshotVersion: number;
}

export interface GitImportSelectionResult {
  operationId: string;
  items: GitImportResult[];
  snapshotVersion: number;
}

export type GitRepositorySourceType = "github" | "gitlab" | "git";

export interface SourceTrackingPolicy {
  mode: string;
  value: string | null;
}

export interface FetchLatestAndManageRequest {
  sourceType: GitRepositorySourceType;
  sourceUrl: string;
  trackingPolicy: SourceTrackingPolicy | null;
}

export interface ExternalOwnershipClaim {
  lockPath: string;
  entryName: string;
  requestedRef: string;
}

export type SourceGroupMemberAction = "added" | "current";

export interface SourceGroupMember {
  pluginName?: string | null;
  directoryName: string;
  displayName: string;
  description: string;
  skillPath: string;
  treeSummary: string;
  action: SourceGroupMemberAction;
}

export interface SourceGroupPolicyFacts {
  mode: string;
  value: string | null;
  selectionKind: string;
  selectedRef: string;
  resolvedCommit: string;
}

export interface SourceGroupPreview {
  provider: string;
  sourceUrl: string;
  aliases: string[];
  policy: SourceGroupPolicyFacts;
  members: SourceGroupMember[];
  externalOwnershipClaims: ExternalOwnershipClaim[];
  removedExternalClaims?: string[];
  addedMemberNames?: string[];
}

export interface RepositoryRefConflict {
  provider: string;
  sourceUrl: string;
  availableRefs: string[];
  externalOwnershipClaims: ExternalOwnershipClaim[];
}

export interface RepositoryOwnershipSplit {
  provider: string;
  sourceUrl: string;
  lockPaths: string[];
  externalOwnershipClaims: ExternalOwnershipClaim[];
}

export type SourceGroupPreviewOutcome =
  | { kind: "preview"; preview: SourceGroupPreview }
  | { kind: "repository_ref_conflict"; conflict: RepositoryRefConflict }
  | { kind: "repository_ownership_split"; split: RepositoryOwnershipSplit };

export interface ConfirmSourceTransitionRequest {
  sourceType: GitRepositorySourceType;
  sourceUrl: string;
  trackingPolicy: SourceTrackingPolicy | null;
  expectedSelectedRef: string;
  expectedResolvedCommit: string;
  expectedRemovedClaims?: string[];
}

export interface SourceTransitionResult {
  operationId: string;
  remoteId: string;
  releaseId: string;
  resolvedCommit: string;
  memberCount: number;
  snapshotVersion: number;
  undoAvailable: boolean;
}

export interface SourceUndoResult {
  operationId: string;
  memberCount: number;
  snapshotVersion: number;
}

export type SourcePromotionMemberState = "added" | "current";

export interface SourcePromotionDraftMember {
  pluginName?: string | null;
  skillPath: string;
  directoryName: string;
  directoryIdentityKey: string;
  displayName: string;
  description: string;
  treeSummary: string;
  state: SourcePromotionMemberState;
}

export interface SourcePromotionRemovedMember {
  skillId: string;
  directoryName: string;
  skillPath: string;
}

export interface SourcePromotionDraft {
  remoteId: string;
  provider: string;
  sourceUrl: string;
  aliases: string[];
  policy: SourceGroupPolicyFacts;
  members: SourcePromotionDraftMember[];
  removedMembers: SourcePromotionRemovedMember[];
  legacyMemberCount: number;
  externalOwnershipClaims: ExternalOwnershipClaim[];
}

export type SourcePromotionDraftOutcome =
  | { kind: "draft"; draft: SourcePromotionDraft }
  | { kind: "repository_ref_conflict"; conflict: RepositoryRefConflict }
  | { kind: "repository_ownership_split"; split: RepositoryOwnershipSplit };

export interface ConfirmSourcePromotionRequest {
  remoteId: string;
  sourceType: GitRepositorySourceType;
  sourceUrl: string;
  trackingPolicy: SourceTrackingPolicy | null;
  expectedSelectedRef: string;
  expectedResolvedCommit: string;
}

export interface SourcePromotionResult {
  operationId: string;
  remoteId: string;
  releaseId: string;
  resolvedCommit: string;
  memberCount: number;
  snapshotVersion: number;
  undoAvailable: boolean;
}

export type SourceUpdateMemberState = "current" | "added" | "removed";

export interface SourceUpdateDraftMember {
  pluginName?: string | null;
  skillId: string;
  skillPath: string;
  directoryName: string;
  directoryIdentityKey: string;
  displayName: string;
  description: string;
  treeSummary: string;
  state: SourceUpdateMemberState;
}

export interface SourceUpdateDraft {
  alreadyCurrent: boolean;
  remoteId: string;
  provider: string;
  sourceUrl: string;
  aliases: string[];
  policy: SourceGroupPolicyFacts;
  members: SourceUpdateDraftMember[];
}

export interface ConfirmSourceUpdateRequest {
  remoteId: string;
  /** The frozen preview facts; a changed ref fails PreviewStale. */
  expectedSelectedRef: string;
  expectedResolvedCommit: string;
}

export type SourceUpdateResult = SourcePromotionResult | null;

export interface SourceRestoreResult {
  remoteId: string;
  restoredMembers: number;
  snapshotVersion: number;
}

export interface SourceLocalCopyResult {
  operationId: string;
  skillId: string;
  directoryName: string;
  destination: string;
  snapshotVersion: number;
}

export interface SourceRemoveResult {
  operationId: string;
  remoteId: string;
  memberCount: number;
  snapshotVersion: number;
}

export interface UpdateCheckItem {
  skillId: string;
  directoryName: string;
  sourceUrl: string;
  requestedRef: string;
  currentCommit: string;
  resolvedCommit: string;
  hasUpdate: boolean;
  modified: boolean;
  upstreamPathGone: boolean;
  lastCheckedAt: string | null;
}

export interface UpdateCheckGroup {
  repoUrl: string;
  items: UpdateCheckItem[];
}

export interface UpdateCheckReport {
  groups: UpdateCheckGroup[];
  errors: string[];
  /** Closed Remote Source Identity Conflicts (parent manifest vs row). */
  parentConflicts: {
    remoteId: string;
    canonicalUrl: string;
  }[];
}

export interface UpdateSelection {
  skillId: string;
  newSkillPath: string | null;
}

export interface UpdatePlanItem {
  skillId: string;
  directoryName: string;
  planToken: string;
  currentCommit: string;
  newCommit: string;
  modified: boolean;
  pathChanged: boolean;
  error: string | null;
}

export interface UpdatePlan {
  items: UpdatePlanItem[];
}

export interface UpdateApplyRequest {
  planToken: string;
  skillId: string;
  directoryName: string;
}

export interface UpdateItemResult {
  skillId: string;
  directoryName: string;
  updated: boolean;
  error: string | null;
}

export interface UpdateResult {
  items: UpdateItemResult[];
}

export interface EvidenceChainHop {
  path: string;
  kind: "directory" | "symlink";
  /** The raw symlink target text for symlink hops; Source Content. */
  target: string | null;
  device: number;
  inode: number;
}

/** One plan-item appearance with its full bounded multi-hop chain (spec
 * §8.1: every hop is presented). */
export interface AdoptAppearanceEvidence {
  entryPath: string;
  kind: "real_directory" | "symlink";
  originalTarget: string | null;
  chain: EvidenceChainHop[];
}

/** One Activation pair of a plan item (entry path → target path). */
export interface AdoptActivation {
  entryPath: string;
  targetPath: string;
}

/**
 * Closed Adopt actions (spec §4.6): the only planable operations of the
 * terminal Scan Report surface are keep-in-place Local Link, explicitly
 * confirmed external Link migration, and an explicit Conflict Set winner.
 */
export type AdoptAction =
  "local_link" | "local_link_with_move" | "conflict_winner";

/** One explicit selection of the report plan request. */
export interface AdoptReportSelection {
  entityRef: string;
  action: AdoptAction;
  destinationParent?: string;
}

export interface AdoptPlanItem {
  entityRef: string;
  action: AdoptAction;
  directoryName: string;
  canonicalEntity: string;
  finalEntityPath: string;
  appearances: AdoptAppearanceEvidence[];
  activations: AdoptActivation[];
  applyable: boolean;
  error: string | null;
}

export interface AdoptPlan {
  planToken: string;
  reportGeneration: number;
  items: AdoptPlanItem[];
  canApply: boolean;
}

export interface AdoptSkillResult {
  skillId: string;
  directoryName: string;
  adopted: boolean;
  error: string | null;
}

export interface AdoptResult {
  operationId: string;
  items: AdoptSkillResult[];
  snapshotVersion: number;
  undoAvailable: boolean;
}

export interface AdoptUndoItemResult {
  directoryName: string;
  undone: boolean;
  error: string | null;
}

export interface AdoptUndoResult {
  operationId: string;
  items: AdoptUndoItemResult[];
  snapshotVersion: number;
}

export interface CommunityUpdate {
  version: string;
  releaseUrl: string;
}

export interface CatalogClient {
  checkCommunityUpdate(): Promise<CommunityUpdate | null>;
  getBootstrapSnapshot(): Promise<BootstrapSnapshot>;
  prepareHome(path: string): Promise<HomeCandidate>;
  prepareExistingHomeRecovery(path: string): Promise<ExistingHomeRecoveryPlan>;
  cancelExistingHomeRecovery(planToken: string): Promise<void>;
  confirmExistingHomeRecovery(planToken: string): Promise<BootstrapSnapshot>;
  confirmHome(candidateToken: string): Promise<BootstrapSnapshot>;
  continueCandidate(operationId: string): Promise<BootstrapSnapshot>;
  cancelCandidate(operationId: string): Promise<BootstrapSnapshot>;
  /** Reconnect Same Home: re-verifies the existing binding (spec §5.5). */
  reconnectSameHome(): Promise<BootstrapSnapshot>;
  planAbandon(): Promise<AbandonPreview>;
  applyAbandon(planToken: string, homeId: string): Promise<BootstrapSnapshot>;
  /** Restore eligibility probe (spec §5.5). */
  getRestoreEligibility(): Promise<RestoreEligibility>;
  planRestore(): Promise<{ planToken: string }>;
  listenBootstrapChanged(
    callback: (payload: BootstrapChangedPayload) => void,
  ): Promise<() => void>;
  getLocaleSnapshot(): Promise<LocaleSnapshot>;
  setLocaleSelection(selection: LocaleSelection): Promise<LocaleSnapshot>;
  /** Re-negotiate the effective locale from the system preferred list. */
  refreshSystemLanguages(): Promise<LocaleSnapshot>;
  listenLocaleChanged(
    callback: (payload: LocaleSnapshot) => void,
  ): Promise<() => void>;
  listSkills(filter: CatalogFilter): Promise<CatalogList>;
  /** Read-only Source Capability Scan (ADR-0014, spec §8.3). */
  getGitSourceCapability(): Promise<GitSourceCapabilityReport>;
  inspectSkill(skillId: string): Promise<SkillDetail>;
  getAgentManagementSnapshot(): Promise<AgentManagementSnapshot>;
  /** Current in-memory Observation snapshot; never triggers detection. */
  /** The current Skill's canonical Target groups (spec §4.9; zero-write). */
  listTargetGroups(skillId: string): Promise<GlobalTargetGroupSnapshot>;
  /** Plan a Global Enable; cells come out in Target order then Skill order. */
  planGlobalEnable(
    skillIds: string[],
    targetGroupIds: string[],
    cellResolutions: CellResolutionRequest[],
  ): Promise<EnablePlan>;
  /** Plan one Global lifecycle action (Enable / Disable / Repair). */
  planGlobalLifecycle(
    skillId: string,
    targetGroupId: string,
    action: EnableAction,
  ): Promise<EnablePlan>;
  applyGlobalEnable(planToken: string): Promise<EnableResult>;
  undoGlobalEnable(operationId: string): Promise<EnableUndoResult>;
  finalizeGlobalEnable(operationId: string): Promise<void>;
  listRecentProjectFolders(): Promise<RecentProjectFolder[]>;
  clearRecentProjectFolders(): Promise<void>;
  planProjectEnable(
    skillIds: string[],
    projectFolder: string,
    agentIds: string[],
    cellResolutions: CellResolutionRequest[],
  ): Promise<EnablePlan>;
  applyProjectEnable(
    planToken: string,
    confirmedCellKeys?: string[],
  ): Promise<EnableResult>;
  undoProjectEnable(operationId: string): Promise<EnableUndoResult>;
  finalizeProjectEnable(operationId: string): Promise<void>;
  getObservationSnapshot(): Promise<ObservationAndScanSnapshot>;
  /** Single-flight Detection trigger (spec §4.10; ADR-0020). */
  refreshDetection(): Promise<ObservationAndScanSnapshot>;
  /** Single-flight Startup Probe trigger (spec §4.10; ADR-0020). */
  refreshStartupProbe(): Promise<ObservationAndScanSnapshot>;
  /** Target-scoped Activation Health trigger (spec §4.10; ADR-0020). */
  refreshActivationHealth(
    targetRootIds?: string[],
  ): Promise<ObservationAndScanSnapshot>;
  /** The unique paged observation read contract (`observationPage`). */
  getObservationPage(
    kind: ObservationKind,
    generation: number,
    cursor: ObservationCursor,
    limit?: number,
  ): Promise<ObservationPage>;
  /** Start a full Rescan Run (single-flight; spec §4.10). */
  startRescan(
    trigger: "onboarding" | "manual",
  ): Promise<ObservationAndScanSnapshot>;
  /** Coordinate cancellation of the active Rescan Run. */
  cancelRescan(runId: string): Promise<ObservationAndScanSnapshot>;
  ignoreScanLocalCandidate(
    reportContentIdentity: string,
    generation: number,
    entitySeq: number,
  ): Promise<void>;
  /** The unique paged Report read contract (spec §4.10 `report_page`). */
  getScanReportPage(
    cursor: ScanReportCursor,
    limit?: number,
  ): Promise<ScanReportPage>;
  /** `observation://changed`: payload isomorphic with the query snapshot. */
  listenObservationChanged(
    callback: (payload: ObservationAndScanSnapshot) => void,
  ): Promise<() => void>;
  planCreateAgentConfiguration(
    draft: AgentConfigurationDraft,
  ): Promise<AgentConfigurationPlan>;
  planEditAgentConfiguration(
    agentId: string,
    draft: AgentConfigurationDraft,
  ): Promise<AgentConfigurationPlan>;
  planDeleteAgentConfiguration(
    agentId: string,
  ): Promise<AgentConfigurationPlan>;
  applyAgentConfigurationPlan(
    planToken: string,
  ): Promise<AgentConfigurationApplyResult>;
  loadPreferences(): Promise<AppPreferences>;
  updatePreferences(
    updates: PreferenceUpdates,
  ): Promise<UpdatePreferencesResult>;
  checkAppUpdate(force: boolean): Promise<AppUpdateCheck>;
  downloadAppUpdate(updateId: string): Promise<DownloadedAppUpdate>;
  cancelAppUpdate(updateId: string): Promise<CancelledAppUpdate>;
  installAppUpdate(updateId: string): Promise<void>;
  startupInfo(): Promise<StartupInfo>;
  completeOnboarding(): Promise<void>;
  createAgentDirectory(agentId: string): Promise<StartupInfo>;
  relocateLink(
    skillId: string,
    sourcePath: string,
  ): Promise<RelocateLinkPreview>;
  applyRelocateLink(planToken: string): Promise<RelocateLinkResult>;
  cancelRelocateLink(planToken: string): Promise<boolean>;
  planRemoveSkill(skillId: string): Promise<RemoveSkillPreview>;
  applyRemoveSkill(planToken: string): Promise<RemoveSkillResult>;
  cancelRemoveSkill(planToken: string): Promise<boolean>;
  discoverLinkImport(sourcePath: string): Promise<LinkImportCandidate>;
  planLinkImport(sourcePath: string): Promise<LinkImportPreview>;
  applyLinkImport(planToken: string): Promise<LinkImportResult>;
  cancelLinkImport(planToken: string): Promise<boolean>;
  fetchLatestAndManage(
    request: FetchLatestAndManageRequest,
  ): Promise<SourceGroupPreviewOutcome>;
  previewSourcePromotion(
    remoteId: string,
    trackingPolicy: SourceTrackingPolicy | null,
  ): Promise<SourcePromotionDraftOutcome>;
  confirmSourcePromotion(
    request: ConfirmSourcePromotionRequest,
  ): Promise<SourcePromotionResult>;
  finalizeSourcePromotion(operationId: string): Promise<void>;
  previewSourceUpdate(remoteId: string): Promise<SourceUpdateDraft>;
  confirmSourceUpdate(
    request: ConfirmSourceUpdateRequest,
  ): Promise<SourceUpdateResult>;
  undoSourceUpdate(operationId: string): Promise<SourceUndoResult>;
  finalizeSourceUpdate(operationId: string): Promise<void>;
  restoreCurrentSourceRelease(remoteId: string): Promise<SourceRestoreResult>;
  createLocalSourceCopy(
    remoteId: string,
    skillId: string,
    destination: string,
  ): Promise<SourceLocalCopyResult>;
  removeGitSource(remoteId: string): Promise<SourceRemoveResult>;
  confirmSourceTransition(
    request: ConfirmSourceTransitionRequest,
  ): Promise<SourceTransitionResult>;
  undoSourceTransition(operationId: string): Promise<SourceUndoResult>;
  finalizeSourceTransition(operationId: string): Promise<void>;
  checkSkillUpdates(force: boolean): Promise<UpdateCheckReport>;
  planSkillUpdates(selections: UpdateSelection[]): Promise<UpdatePlan>;
  applySkillUpdates(
    requests: UpdateApplyRequest[],
    abandonChanges: boolean,
  ): Promise<UpdateResult>;
  pinSkillUpdates(skillIds: string[]): Promise<void>;
  planAdopt(
    reportGeneration: number,
    selections: AdoptReportSelection[],
  ): Promise<AdoptPlan>;
  applyAdopt(planToken: string): Promise<AdoptResult>;
  undoAdopt(operationId: string): Promise<AdoptUndoResult>;
  finalizeAdopt(operationId: string): Promise<void>;
  cancelAdopt(planToken: string): Promise<boolean>;
  getFixtureRecoveryPreview(): Promise<FixtureRecoveryPreview>;
  planFixtureRecovery(): Promise<{ planToken: string }>;
  applyFixtureRecovery(planToken: string): Promise<RecoveryResult>;
  confirmFixtureRecoveryResult(operationId: string): Promise<BootstrapSnapshot>;
  listSafetySnapshots(): Promise<SafetySnapshot[]>;
  planDeleteSafetySnapshot(snapshotId: string): Promise<DeleteSnapshotPreview>;
  applyDeleteSafetySnapshot(planToken: string): Promise<void>;
}

const tauriCatalogClient: CatalogClient = {
  checkCommunityUpdate() {
    return invoke<CommunityUpdate | null>("check_community_update");
  },
  getBootstrapSnapshot() {
    return invoke<BootstrapSnapshot>("get_bootstrap_snapshot");
  },
  prepareHome(path) {
    return invoke<HomeCandidate>("prepare_home", { request: { path } });
  },
  prepareExistingHomeRecovery(path) {
    return invoke<ExistingHomeRecoveryPlan>("prepare_existing_home_recovery", {
      request: { path },
    });
  },
  cancelExistingHomeRecovery(planToken) {
    return invoke<void>("cancel_existing_home_recovery", {
      request: { planToken },
    });
  },
  confirmExistingHomeRecovery(planToken) {
    return invoke<BootstrapSnapshot>("confirm_existing_home_recovery", {
      request: { planToken },
    });
  },
  confirmHome(candidateToken) {
    return invoke<BootstrapSnapshot>("confirm_home", {
      request: { candidateToken },
    });
  },
  continueCandidate(operationId) {
    return invoke<BootstrapSnapshot>("continue_candidate", {
      request: { operationId },
    });
  },
  cancelCandidate(operationId) {
    return invoke<BootstrapSnapshot>("cancel_candidate", {
      request: { operationId },
    });
  },
  reconnectSameHome() {
    return invoke<BootstrapSnapshot>("reconnect_same_home");
  },
  planAbandon() {
    return invoke<AbandonPreview>("plan_abandon");
  },
  applyAbandon(planToken, homeId) {
    return invoke<BootstrapSnapshot>("apply_abandon", {
      request: { planToken, homeId },
    });
  },
  getRestoreEligibility() {
    return invoke<RestoreEligibility>("restore_eligibility");
  },
  planRestore() {
    return invoke<{ planToken: string }>("plan_restore");
  },
  listenBootstrapChanged(callback) {
    return listen<BootstrapChangedPayload>("bootstrap://changed", (event) => {
      callback(event.payload);
    });
  },
  getLocaleSnapshot() {
    return invoke<LocaleSnapshot>("get_locale_snapshot");
  },
  setLocaleSelection(selection) {
    return invoke<LocaleSnapshot>("set_locale_selection", {
      request: { selection },
    });
  },
  refreshSystemLanguages() {
    return invoke<LocaleSnapshot>("refresh_system_languages");
  },
  listenLocaleChanged(callback) {
    return listen<LocaleSnapshot>("locale://changed", (event) => {
      callback(event.payload);
    });
  },
  listenObservationChanged(callback) {
    return listen<ObservationAndScanSnapshot>(
      "observation://changed",
      (event) => {
        callback(event.payload);
      },
    );
  },
  listSkills(filter) {
    return invoke<CatalogList>("list_skills", { request: { filter } });
  },
  getGitSourceCapability() {
    return invoke<GitSourceCapabilityReport>("get_git_source_capability");
  },
  relocateLink(skillId, sourcePath) {
    return invoke<RelocateLinkPreview>("relocate_link", {
      request: { skillId, sourcePath },
    });
  },
  applyRelocateLink(planToken) {
    return invoke<RelocateLinkResult>("apply_relocate_link", {
      request: { planToken },
    });
  },
  cancelRelocateLink(planToken) {
    return invoke<boolean>("cancel_relocate_link", {
      request: { planToken },
    });
  },
  planRemoveSkill(skillId) {
    return invoke<RemoveSkillPreview>("plan_remove_skill", {
      request: { skillId },
    });
  },
  applyRemoveSkill(planToken) {
    return invoke<RemoveSkillResult>("apply_remove_skill", {
      request: { planToken },
    });
  },
  cancelRemoveSkill(planToken) {
    return invoke<boolean>("cancel_remove_skill", {
      request: { planToken },
    });
  },
  inspectSkill(skillId) {
    return invoke<SkillDetail>("inspect_skill", { skillId });
  },
  getAgentManagementSnapshot() {
    return invoke<AgentManagementSnapshot>("get_agent_management_snapshot");
  },
  getObservationSnapshot() {
    return invoke<ObservationAndScanSnapshot>("get_observation_snapshot");
  },
  refreshDetection() {
    return invoke<ObservationAndScanSnapshot>("refresh_detection");
  },
  refreshStartupProbe() {
    return invoke<ObservationAndScanSnapshot>("refresh_startup_probe");
  },
  refreshActivationHealth(targetRootIds) {
    return invoke<ObservationAndScanSnapshot>("refresh_activation_health", {
      request: { targetRootIds: targetRootIds ?? null },
    });
  },
  getObservationPage(kind, generation, cursor, limit) {
    return invoke<ObservationPage>("get_observation_page", {
      request: { kind, generation, cursor, limit: limit ?? 64 },
    });
  },
  startRescan(trigger) {
    return invoke<ObservationAndScanSnapshot>("start_rescan", {
      request: { trigger },
    });
  },
  cancelRescan(runId) {
    return invoke<ObservationAndScanSnapshot>("cancel_rescan", {
      request: { runId },
    });
  },
  getScanReportPage(cursor, limit) {
    return invoke<ScanReportPage>("get_scan_report_page", {
      request: { cursor, limit: limit ?? 64 },
    });
  },
  ignoreScanLocalCandidate(reportContentIdentity, generation, entitySeq) {
    return invoke<void>("ignore_scan_local_candidate", {
      reportContentIdentity,
      generation,
      entitySeq,
    });
  },
  listTargetGroups(skillId) {
    return invoke<GlobalTargetGroupSnapshot>("list_target_groups", { skillId });
  },
  planGlobalEnable(skillIds, targetGroupIds, cellResolutions) {
    return invoke<EnablePlan>("plan_global_enable", {
      request: { skillIds, targetGroupIds, cellResolutions },
    });
  },
  planGlobalLifecycle(skillId, targetGroupId, action) {
    return invoke<EnablePlan>("plan_global_lifecycle", {
      request: { skillId, targetGroupId, action },
    });
  },
  applyGlobalEnable(planToken) {
    return invoke<EnableResult>("apply_global_enable", {
      request: { planToken },
    });
  },
  undoGlobalEnable(operationId) {
    return invoke<EnableUndoResult>("undo_global_enable", {
      request: { operationId },
    });
  },
  finalizeGlobalEnable(operationId) {
    return invoke<void>("finalize_global_enable", {
      request: { operationId },
    });
  },
  listRecentProjectFolders() {
    return invoke<RecentProjectFolder[]>("list_recent_project_folders");
  },
  clearRecentProjectFolders() {
    return invoke<void>("clear_recent_project_folders");
  },
  planProjectEnable(skillIds, projectFolder, agentIds, cellResolutions) {
    return invoke<EnablePlan>("plan_project_enable", {
      request: { skillIds, projectFolder, agentIds, cellResolutions },
    });
  },
  applyProjectEnable(planToken, confirmedCellKeys = []) {
    return invoke<EnableResult>("apply_project_enable", {
      request: { planToken, confirmedCellKeys },
    });
  },
  undoProjectEnable(operationId) {
    return invoke<EnableUndoResult>("undo_project_enable", {
      request: { operationId },
    });
  },
  finalizeProjectEnable(operationId) {
    return invoke<void>("finalize_project_enable", {
      request: { operationId },
    });
  },
  planCreateAgentConfiguration(draft) {
    return invoke<AgentConfigurationPlan>("plan_create_agent_configuration", {
      request: draft,
    });
  },
  planEditAgentConfiguration(agentId, draft) {
    return invoke<AgentConfigurationPlan>("plan_edit_agent_configuration", {
      request: { agentId, ...draft },
    });
  },
  planDeleteAgentConfiguration(agentId) {
    return invoke<AgentConfigurationPlan>("plan_delete_agent_configuration", {
      request: { agentId },
    });
  },
  applyAgentConfigurationPlan(planToken) {
    return invoke<AgentConfigurationApplyResult>(
      "apply_agent_configuration_plan",
      { request: { planToken } },
    );
  },
  loadPreferences() {
    return invoke<AppPreferences>("load_preferences");
  },
  updatePreferences(updates) {
    return invoke<UpdatePreferencesResult>("update_preferences", {
      request: updates,
    });
  },
  checkAppUpdate(force) {
    return invoke<AppUpdateCheck>("check_app_update", {
      request: { force },
    });
  },
  downloadAppUpdate(updateId) {
    return invoke<DownloadedAppUpdate>("download_app_update", {
      request: { updateId },
    });
  },
  cancelAppUpdate(updateId) {
    return invoke<CancelledAppUpdate>("cancel_app_update", {
      request: { updateId },
    });
  },
  installAppUpdate(updateId) {
    return invoke<void>("install_app_update", {
      request: { updateId },
    });
  },
  startupInfo() {
    return invoke<StartupInfo>("startup_info");
  },
  completeOnboarding() {
    return invoke<void>("complete_onboarding");
  },
  createAgentDirectory(agentId) {
    return invoke<StartupInfo>("create_agent_directory", {
      request: { agentId },
    });
  },
  discoverLinkImport(sourcePath) {
    return invoke<LinkImportCandidate>("discover_link_import", {
      request: { sourcePath },
    });
  },
  planLinkImport(sourcePath) {
    return invoke<LinkImportPreview>("plan_link_import", {
      request: { sourcePath },
    });
  },
  applyLinkImport(planToken) {
    return invoke<LinkImportResult>("apply_link_import", {
      request: { planToken },
    });
  },
  cancelLinkImport(planToken) {
    return invoke<boolean>("cancel_link_import", {
      request: { planToken },
    });
  },
  fetchLatestAndManage(request) {
    return invoke<SourceGroupPreviewOutcome>("fetch_latest_and_manage", {
      request,
    });
  },
  previewSourcePromotion(remoteId, trackingPolicy) {
    return invoke<SourcePromotionDraftOutcome>("preview_source_promotion", {
      request: { remoteId, trackingPolicy },
    });
  },
  confirmSourcePromotion(request) {
    return invoke<SourcePromotionResult>("confirm_source_promotion", {
      request,
    });
  },
  finalizeSourcePromotion(operationId) {
    return invoke<void>("finalize_source_promotion", {
      request: { operationId },
    });
  },
  previewSourceUpdate(remoteId) {
    return invoke<SourceUpdateDraft>("preview_source_update", {
      request: { remoteId },
    });
  },
  confirmSourceUpdate(request) {
    return invoke<SourceUpdateResult>("confirm_source_update", { request });
  },
  undoSourceUpdate(operationId) {
    return invoke<SourceUndoResult>("undo_source_update", {
      request: { operationId },
    });
  },
  finalizeSourceUpdate(operationId) {
    return invoke<void>("finalize_source_update", {
      request: { operationId },
    });
  },
  restoreCurrentSourceRelease(remoteId) {
    return invoke<SourceRestoreResult>("restore_current_source_release", {
      request: { remoteId },
    });
  },
  createLocalSourceCopy(remoteId, skillId, destination) {
    return invoke<SourceLocalCopyResult>("create_local_source_copy", {
      request: { remoteId, skillId, destination },
    });
  },
  removeGitSource(remoteId) {
    return invoke<SourceRemoveResult>("remove_git_source", {
      request: { remoteId },
    });
  },
  confirmSourceTransition(request) {
    return invoke<SourceTransitionResult>("confirm_source_transition", {
      request,
    });
  },
  undoSourceTransition(operationId) {
    return invoke<SourceUndoResult>("undo_source_transition", {
      request: { operationId },
    });
  },
  finalizeSourceTransition(operationId) {
    return invoke<void>("finalize_source_transition", {
      request: { operationId },
    });
  },
  checkSkillUpdates(force) {
    return invoke<UpdateCheckReport>("check_skill_updates", {
      request: { force },
    });
  },
  planSkillUpdates(selections) {
    return invoke<UpdatePlan>("plan_skill_updates", {
      request: { selections },
    });
  },
  applySkillUpdates(requests, abandonChanges) {
    return invoke<UpdateResult>("apply_skill_updates", {
      request: { requests, abandonChanges },
    });
  },
  pinSkillUpdates(skillIds) {
    return invoke<void>("pin_skill_updates", {
      request: { skillIds },
    });
  },
  planAdopt(reportGeneration, selections) {
    return invoke<AdoptPlan>("plan_adopt", {
      request: { reportGeneration, selections },
    });
  },
  applyAdopt(planToken) {
    return invoke<AdoptResult>("apply_adopt", { request: { planToken } });
  },
  undoAdopt(operationId) {
    return invoke<AdoptUndoResult>("undo_adopt", { request: { operationId } });
  },
  finalizeAdopt(operationId) {
    return invoke<void>("finalize_adopt", { request: { operationId } });
  },
  cancelAdopt(planToken) {
    return invoke<boolean>("cancel_adopt", { request: { planToken } });
  },
  getFixtureRecoveryPreview() {
    return invoke<FixtureRecoveryPreview>("get_fixture_recovery_preview");
  },
  planFixtureRecovery() {
    return invoke<{ planToken: string }>("plan_fixture_recovery", {
      request: {},
    });
  },
  applyFixtureRecovery(planToken) {
    return invoke<RecoveryResult>("apply_fixture_recovery", {
      request: { planToken },
    });
  },
  confirmFixtureRecoveryResult(operationId) {
    return invoke<BootstrapSnapshot>("confirm_fixture_recovery_result", {
      request: { operationId },
    });
  },
  listSafetySnapshots() {
    return invoke<SafetySnapshot[]>("list_safety_snapshots");
  },
  planDeleteSafetySnapshot(snapshotId) {
    return invoke<DeleteSnapshotPreview>("plan_delete_safety_snapshot", {
      request: { planToken: snapshotId },
    });
  },
  applyDeleteSafetySnapshot(planToken) {
    return invoke<void>("apply_delete_safety_snapshot", {
      request: { planToken },
    });
  },
};

export function createCatalogClient(): CatalogClient {
  // Production never falls back to fixture data (spec §2.2, §10.1): outside
  // the Tauri runtime every catalog call fails closed and the bootstrap
  // snapshot reports a closed failure. Tests and prototypes inject
  // `createFixtureCatalogClient()` explicitly.
  return "__TAURI_INTERNALS__" in window
    ? tauriCatalogClient
    : createClosedBootstrapClient();
}

const CLOSED_ERROR = {
  code: "bootstrap_unavailable",
  message: "Skill Man is not running in the Tauri runtime.",
};

/**
 * The non-Tauri production client: closed bootstrap failure for the
 * snapshot, a rejected closed error for every catalog command.
 */
function createClosedBootstrapClient(): CatalogClient {
  const closed = () => Promise.reject(CLOSED_ERROR);
  const client = {} as CatalogClient;
  for (const key of Object.keys(tauriCatalogClient) as Array<
    keyof CatalogClient
  >) {
    (client as unknown as Record<string, unknown>)[key] = closed;
  }
  client.getBootstrapSnapshot = async () => ({
    state: "app_state_unavailable",
    diagnostic: {
      code: "no_tauri_runtime",
      message: "Skill Man is not running in the Tauri runtime.",
    },
  });
  client.listenBootstrapChanged = async () => () => {};
  client.getLocaleSnapshot = async () => ({
    selection: "system",
    effectiveLocale: "en",
    generation: 0,
    diagnostic: null,
  });
  client.setLocaleSelection = async (selection) => ({
    selection,
    effectiveLocale: selection === "zh-Hans" ? "zh-Hans" : "en",
    generation: 1,
    diagnostic: null,
  });
  client.refreshSystemLanguages = async () => ({
    selection: "system",
    effectiveLocale: "en",
    generation: 0,
    diagnostic: null,
  });
  client.listenLocaleChanged = async () => () => {};
  client.checkCommunityUpdate = async () => null;
  return client;
}

/** Apply only the app's native chrome appearance; never changes macOS settings. */
export async function applyAppAppearance(
  appearance: "system" | "light" | "dark",
): Promise<void> {
  if (isTauri()) await invoke<void>("set_app_appearance", { appearance });
}
