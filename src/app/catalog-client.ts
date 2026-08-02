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
  skillDirectoryName: string;
  agentName: string;
  enabled: boolean;
  entryPath: string;
  targetPath: string;
}

export interface ActivationResult {
  skillId: string;
  agentId: string;
  desiredEnabled: boolean;
  observedState: ActivationObservedState;
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
  applyActivation(planToken: string): Promise<ActivationResult>;
  cancelActivation(planToken: string): Promise<boolean>;
}

const tauriCatalogClient: CatalogClient = {
  listSkills(filter) {
    return invoke<CatalogList>("list_skills", { request: { filter } });
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
};

export function createCatalogClient(): CatalogClient {
  return "__TAURI_INTERNALS__" in window
    ? tauriCatalogClient
    : createFixtureCatalogClient();
}
