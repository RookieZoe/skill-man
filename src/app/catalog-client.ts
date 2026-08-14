import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type CatalogFilter = "all" | "broken" | "modified" | "link" | "install";
export type SourceKind = "link" | "remote_install" | "file_install";
export type Health = "healthy" | "broken" | "modified";
export type AgentKind = "claude_preset" | "codex_preset" | "custom";
export type Compatibility = "verified" | "unknown";
export type ActivationObservedState =
  "present" | "missing" | "target_mismatch" | "dangling" | "occupied";

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

export interface BootstrapDiagnostic {
  code: string;
  message: string;
}

/**
 * The closed top-level bootstrap route union (spec §4.2). React renders
 * exactly one route per `state`; no variant is composed from booleans.
 */
export type BootstrapSnapshot =
  | { state: "app_state_unavailable"; diagnostic: BootstrapDiagnostic | null }
  | { state: "unconfigured" }
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

/** The closed public error union (spec §4.7): presentation maps `code` to a
 * message key; typed fields carry Source Content only. */
export type PublicError =
  | { code: "validation" }
  | { code: "not_found" }
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
  | { code: "candidate_invalid"; reason: string }
  | { code: "binding_step_failed"; cursor: string }
  | { code: "binding_state_ambiguous" }
  | { code: "binding_not_cancellable" }
  | { code: "binding_migration_failed" }
  | { code: "locale_store_unavailable" }
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

export interface AgentActivation {
  id: string;
  name: string;
  kind: AgentKind;
  skillsPath: string;
  detected: boolean;
  compatibility: Compatibility;
  desiredEnabled: boolean;
  observedState: ActivationObservedState;
}

export interface ActivationPreview {
  planToken: string;
  skillId: string;
  agentId: string;
  skillDirectoryName: string;
  agentName: string;
  enabled: boolean;
  kind: "enable" | "disable" | "repair";
  entryPath: string;
  targetPath: string;
  compatibilityWarning: CompatibilityWarning | null;
}

export type CompatibilityWarning =
  | { kind: "custom_unknown" }
  | {
      kind: "frontmatter_mismatch";
      frontmatterName: string;
      directoryName: string;
    };

export type OccupierKind = "real_directory" | "symlink" | "file";

export interface OccupierSummary {
  kind: OccupierKind;
  symlinkTarget: string | null;
  finalEntityPath: string | null;
  directoryName: string;
  isSkill: boolean;
  adoptable: boolean;
  notAdoptableReason: OccupierNotAdoptableReason | null;
}

export type OccupierNotAdoptableReason =
  | { kind: "regular_file" }
  | { kind: "points_at_managed_skill" }
  | { kind: "points_at_this_skill" }
  | { kind: "no_readable_skill_md" }
  | { kind: "target_unresolvable" }
  | { kind: "identity_conflict"; directoryName: string };

export interface ActivationConflictDetails {
  skillId: string;
  agentId: string;
  entryPath: string;
  targetPath: string;
  occupier: OccupierSummary;
}

export interface ActivationReplacePreview {
  planToken: string;
  operationId: string;
  skillDirectoryName: string;
  agentName: string;
  entryPath: string;
  targetPath: string;
  backupPath: string;
  occupantKind: OccupierKind;
}

export interface ActivationReplaceUndoResult {
  undone: boolean;
  error: string | null;
  snapshotVersion: number;
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
  agents: StartupAgent[];
}

export interface ActivationHealthReport {
  checked: number;
  snapshotVersion: number;
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

export interface ActivationResult {
  skillId: string;
  agentId: string;
  desiredEnabled: boolean;
  observedState: ActivationObservedState;
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

export type AdoptVerdict =
  | "local"
  | "verified"
  | "modified"
  | "conflict"
  | "deferred"
  | "blocked"
  | "excluded";

export type ChainFault =
  | { kind: "dangling"; at: string }
  | { kind: "cycle"; at: string }
  | { kind: "hop_limit"; at: string }
  | { kind: "non_utf8"; at: string }
  | { kind: "read_failed"; at: string; detail: string }
  | { kind: "not_directory"; at: string }
  | { kind: "identity_replaced"; at: string };

export type LockFileFault =
  | { kind: "not_utf8" }
  | { kind: "invalid_json"; detail: string }
  | { kind: "unsupported_version"; version: number }
  | { kind: "duplicate_key"; key: string };

export type AdoptVerdictReason =
  | { kind: "no_lock" }
  | { kind: "duplicate_lock_owner"; otherLockPath: string }
  | { kind: "lock_file_fault"; lockPath: string; fault: LockFileFault }
  | { kind: "lock_entry_fault"; lockPath: string; reason: string }
  | { kind: "entity_not_at_installer_root"; expected: string }
  | { kind: "remote_conflict"; detail: string }
  | { kind: "identity_conflict"; names: string[] }
  | { kind: "library_conflict"; directoryName: string }
  | { kind: "remote_unavailable"; detail: string }
  | { kind: "chain_fault"; fault: ChainFault }
  | { kind: "unreadable_entity"; detail: string }
  | { kind: "fixture_entity" };

export interface EvidenceChainHop {
  path: string;
  kind: string;
  device: number;
  inode: number;
}

export interface EvidenceChain {
  entryPath: string;
  entryDevice: number;
  entryInode: number;
  hops: EvidenceChainHop[];
  finalEntity: string | null;
  fault: ChainFault | null;
}

export interface AdoptAppearanceEvidence {
  entryPath: string;
  kind: "real_directory" | "symlink";
  agentId: string | null;
  shared: boolean;
  originalTarget: string | null;
  chain: EvidenceChain;
}

/** The strict lock entry as raw Source Content (spec §6.3). */
export interface LockEntry {
  name: string;
  sourceType: string;
  source: string;
  sourceUrl: string;
  requestedRef: string | null;
  skillPath: string;
  skillFolderHash: string;
  installedAt: string | null;
  updatedAt: string | null;
  pluginName: string | null;
}

export interface AdoptLockEvidence {
  lockPath: string;
  lockFingerprint: string;
  entryName: string;
  entry: LockEntry | null;
  entryFault: string | null;
  fileFault: LockFileFault | null;
}

export interface AdoptRemoteEvidence {
  canonicalUrl: string;
  requestedRef: string;
  refKind: string;
  anchorCommit: string;
  originalInstallCommitKnown: boolean;
  skillPath: string;
  providerHash: string;
  providerHashMatched: boolean;
  remoteTreeHash: string;
  localTreeHash: string;
  treesMatch: boolean;
  defaultBranch: string | null;
}

export interface AdoptLockFile {
  path: string;
  fingerprint: string;
  byteLen: number;
  version: number;
  fault: LockFileFault | null;
  entryNames: string[];
  entryFaults: { name: string; reason: string }[];
}

export interface AdoptEvidenceCandidate {
  canonicalEntity: string;
  directoryName: string;
  directoryNames: string[];
  appearances: AdoptAppearanceEvidence[];
  verdict: AdoptVerdict;
  reason: AdoptVerdictReason | null;
  lock: AdoptLockEvidence | null;
  remote: AdoptRemoteEvidence | null;
  localTreeHash: string | null;
  requiresRelocation: boolean;
  selectable: boolean;
  adoptable: boolean;
  conflict: LibraryConflict | null;
  suggestedAgentIds: string[];
}

export interface AdoptEvidenceReport {
  generation: number;
  candidates: AdoptEvidenceCandidate[];
  lockFiles: AdoptLockFile[];
  truncated: boolean;
}

export type ModifiedBranch =
  | "keep_current"
  | "discard_to_anchor"
  | "convert_to_local_link";

export interface AdoptSelection {
  canonicalEntity: string;
  agentIds: string[];
  modifiedBranch?: ModifiedBranch;
}

export interface AdoptTargetAgent {
  agentId: string;
  name: string;
}

export type AdoptPlanIntent =
  | "local_link"
  | "local_link_with_move"
  | "remote_install_keep_current"
  | "remote_install_discard_modified"
  | "remote_install_convert_to_link";

export interface AdoptPlanItem {
  directoryName: string;
  canonicalEntity: string;
  intent: AdoptPlanIntent;
  finalEntityPath: string;
  appearances: AdoptAppearanceEvidence[];
  targetAgents: AdoptTargetAgent[];
  applyable: boolean;
  error: string | null;
}

export interface AdoptPlan {
  planToken: string;
  evidenceGeneration: number;
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

export interface CatalogClient {
  getBootstrapSnapshot(): Promise<BootstrapSnapshot>;
  prepareHome(path: string): Promise<HomeCandidate>;
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
  inspectSkill(skillId: string): Promise<SkillDetail>;
  listAgents(skillId: string): Promise<AgentActivation[]>;
  planActivation(
    skillId: string,
    agentId: string,
    enabled: boolean,
  ): Promise<ActivationPreview>;
  planActivationRepair(
    skillId: string,
    agentId: string,
  ): Promise<ActivationPreview>;
  activationConflictDetails(
    skillId: string,
    agentId: string,
  ): Promise<ActivationConflictDetails>;
  planActivationReplace(
    skillId: string,
    agentId: string,
  ): Promise<ActivationReplacePreview>;
  applyActivationReplace(planToken: string): Promise<ActivationResult>;
  cancelActivationReplace(planToken: string): Promise<boolean>;
  undoActivationReplace(
    operationId: string,
  ): Promise<ActivationReplaceUndoResult>;
  finalizeActivationReplace(operationId: string): Promise<void>;
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
  runActivationHealthCheck(): Promise<ActivationHealthReport>;
  relocateLink(
    skillId: string,
    sourcePath: string,
  ): Promise<RelocateLinkPreview>;
  applyRelocateLink(planToken: string): Promise<RelocateLinkResult>;
  cancelRelocateLink(planToken: string): Promise<boolean>;
  planRemoveSkill(skillId: string): Promise<RemoveSkillPreview>;
  applyRemoveSkill(planToken: string): Promise<RemoveSkillResult>;
  cancelRemoveSkill(planToken: string): Promise<boolean>;
  applyActivation(planToken: string): Promise<ActivationResult>;
  cancelActivation(planToken: string): Promise<boolean>;
  discoverLinkImport(sourcePath: string): Promise<LinkImportCandidate>;
  planLinkImport(sourcePath: string): Promise<LinkImportPreview>;
  applyLinkImport(planToken: string): Promise<LinkImportResult>;
  cancelLinkImport(planToken: string): Promise<boolean>;
  discoverGitImport(
    source: string,
    forceFullDepth: boolean,
  ): Promise<GitImportDiscovery>;
  planGitImportSelection(
    source: string,
    forceFullDepth: boolean,
    selectedDirectoryNames: string[],
  ): Promise<GitImportSelectionPreview>;
  applyGitImportSelection(planToken: string): Promise<GitImportSelectionResult>;
  cancelGitImportSelection(planToken: string): Promise<boolean>;
  checkSkillUpdates(force: boolean): Promise<UpdateCheckReport>;
  planSkillUpdates(selections: UpdateSelection[]): Promise<UpdatePlan>;
  applySkillUpdates(
    requests: UpdateApplyRequest[],
    abandonChanges: boolean,
  ): Promise<UpdateResult>;
  pinSkillUpdates(skillIds: string[]): Promise<void>;
  scanAdopt(): Promise<AdoptEvidenceReport>;
  planAdopt(
    evidenceGeneration: number,
    selections: AdoptSelection[],
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

let startupHealthCheck: Promise<ActivationHealthReport> | null = null;

const tauriCatalogClient: CatalogClient = {
  getBootstrapSnapshot() {
    return invoke<BootstrapSnapshot>("get_bootstrap_snapshot");
  },
  prepareHome(path) {
    return invoke<HomeCandidate>("prepare_home", { request: { path } });
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
  listSkills(filter) {
    return invoke<CatalogList>("list_skills", { request: { filter } });
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
  listAgents(skillId) {
    return invoke<AgentActivation[]>("list_agents", { skillId });
  },
  planActivation(skillId, agentId, enabled) {
    return invoke<ActivationPreview>("plan_activation", {
      request: { skillId, agentId, enabled },
    });
  },
  planActivationRepair(skillId, agentId) {
    return invoke<ActivationPreview>("plan_activation_repair", {
      request: { skillId, agentId },
    });
  },
  activationConflictDetails(skillId, agentId) {
    return invoke<ActivationConflictDetails>("activation_conflict_details", {
      request: { skillId, agentId },
    });
  },
  planActivationReplace(skillId, agentId) {
    return invoke<ActivationReplacePreview>("plan_activation_replace", {
      request: { skillId, agentId },
    });
  },
  applyActivationReplace(planToken) {
    return invoke<ActivationResult>("apply_activation_replace", {
      request: { planToken },
    });
  },
  cancelActivationReplace(planToken) {
    return invoke<boolean>("cancel_activation_replace", {
      request: { planToken },
    });
  },
  undoActivationReplace(operationId) {
    return invoke<ActivationReplaceUndoResult>("undo_activation_replace", {
      request: { operationId },
    });
  },
  finalizeActivationReplace(operationId) {
    return invoke<void>("finalize_activation_replace", {
      request: { operationId },
    });
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
  runActivationHealthCheck() {
    startupHealthCheck ??= invoke<ActivationHealthReport>(
      "run_activation_health_check",
    ).finally(() => {
      startupHealthCheck = null;
    });
    return startupHealthCheck;
  },
  applyActivation(planToken) {
    return invoke<ActivationResult>("apply_activation", {
      request: { planToken },
    });
  },
  cancelActivation(planToken) {
    return invoke<boolean>("cancel_activation", {
      request: { planToken },
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
  discoverGitImport(source, forceFullDepth) {
    return invoke<GitImportDiscovery>("discover_git_import", {
      request: { source, forceFullDepth },
    });
  },
  planGitImportSelection(source, forceFullDepth, selectedDirectoryNames) {
    return invoke<GitImportSelectionPreview>("plan_git_import_selection", {
      request: { source, forceFullDepth, selectedDirectoryNames },
    });
  },
  applyGitImportSelection(planToken) {
    return invoke<GitImportSelectionResult>("apply_git_import_selection", {
      request: { planToken },
    });
  },
  cancelGitImportSelection(planToken) {
    return invoke<boolean>("cancel_git_import_selection", {
      request: { planToken },
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
  scanAdopt() {
    return invoke<AdoptEvidenceReport>("scan_adopt");
  },
  planAdopt(evidenceGeneration, selections) {
    return invoke<AdoptPlan>("plan_adopt", {
      request: { evidenceGeneration, selections },
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
  return client;
}
