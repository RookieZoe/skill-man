import { invoke } from "@tauri-apps/api/core";

import { createFixtureCatalogClient } from "../test-fixtures/catalog";

export type CatalogFilter = "all" | "broken" | "modified" | "link" | "install";
export type SourceKind = "link" | "remote_install" | "file_install";
export type Health = "healthy" | "broken" | "modified";
export type AgentKind = "claude_preset" | "codex_preset" | "custom";
export type Compatibility = "verified" | "unknown";
export type ActivationObservedState =
  "present" | "missing" | "target_mismatch" | "dangling" | "occupied";

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
  sourceLabel: string;
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
  compatibilityWarning: string | null;
}

export type OccupierKind = "real_directory" | "symlink" | "file";

export interface OccupierSummary {
  kind: OccupierKind;
  symlinkTarget: string | null;
  finalEntityPath: string | null;
  directoryName: string;
  isSkill: boolean;
  adoptable: boolean;
  notAdoptableReason: string | null;
}

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
  warning: string | null;
}

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

export type AdoptRisk = "none" | "external" | "broken";

export interface AdoptAppearance {
  entryPath: string;
  kind: "real_directory" | "symlink";
  agentId: string | null;
  shared: boolean;
}

export interface AdoptCandidate {
  canonicalEntity: string;
  directoryName: string;
  directoryNames: string[];
  appearances: AdoptAppearance[];
  risk: AdoptRisk;
  riskReason: string | null;
  conflict: LibraryConflict | null;
  adoptable: boolean;
  suggestedAgentIds: string[];
}

export interface AdoptScanReport {
  candidates: AdoptCandidate[];
  truncated: boolean;
}

export interface AdoptSelection {
  canonicalEntity: string;
  agentIds: string[];
}

export interface AdoptTargetAgent {
  agentId: string;
  name: string;
}

export interface AdoptPlanItem {
  directoryName: string;
  canonicalEntity: string;
  kind: "migrate" | "link";
  finalEntityPath: string;
  appearances: AdoptAppearance[];
  targetAgents: AdoptTargetAgent[];
  adoptable: boolean;
  error: string | null;
}

export interface AdoptPlan {
  planToken: string;
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
  scanAdopt(): Promise<AdoptScanReport>;
  planAdopt(selections: AdoptSelection[]): Promise<AdoptPlan>;
  applyAdopt(planToken: string): Promise<AdoptResult>;
  undoAdopt(operationId: string): Promise<AdoptUndoResult>;
  finalizeAdopt(operationId: string): Promise<void>;
  cancelAdopt(planToken: string): Promise<boolean>;
}

let startupHealthCheck: Promise<ActivationHealthReport> | null = null;

const tauriCatalogClient: CatalogClient = {
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
    return invoke<AdoptScanReport>("scan_adopt");
  },
  planAdopt(selections) {
    return invoke<AdoptPlan>("plan_adopt", { request: { selections } });
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
};

export function createCatalogClient(): CatalogClient {
  return "__TAURI_INTERNALS__" in window
    ? tauriCatalogClient
    : createFixtureCatalogClient();
}
