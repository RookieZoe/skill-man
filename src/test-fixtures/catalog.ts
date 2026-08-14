import fixtureJson from "../../fixtures/library-desk.json";

import type {
  ActivationPreview,
  ActivationReplacePreview,
  AgentActivation,
  AgentKind,
  AppPreferences,
  CatalogClient,
  CatalogFilter,
  Compatibility,
  Health,
  LinkImportCandidate,
  LinkImportPreview,
  LocaleSelection,
  LocaleSnapshot,
  SkillDetail,
  SourceKind,
  ActivationObservedState,
} from "../app/catalog-client";

type FixtureSkill = Omit<
  SkillDetail,
  "enabledAgentCount" | "fileSourceOriginalPath"
> & {
  fileSourceOriginalPath?: string | null;
};

interface FixtureAgent {
  id: string;
  name: string;
  kind: AgentKind;
  skillsPath: string;
  detected: boolean;
  compatibility: Compatibility;
  enabledSkillIds: string[];
  observedSkillStates?: Record<string, ActivationObservedState>;
}

interface FixtureFile {
  snapshotVersion: number;
  skills: FixtureSkill[];
  agents: FixtureAgent[];
}

interface PlannedFixtureActivation extends ActivationPreview {
  skillId: string;
  agentId: string;
}

interface PlannedFixtureLinkImport extends LinkImportPreview {
  skillId: string;
}

type PlannedFixtureReplace = ActivationReplacePreview & {
  skillId: string;
  agentId: string;
};

const fixture = fixtureJson as FixtureFile;

export function createFixtureCatalogClient(): CatalogClient {
  let snapshotVersion = fixture.snapshotVersion;
  let nextPlanId = 1;
  let firstRunCompleted = true;
  let localeSelection: LocaleSelection = "system";
  let localeGeneration = 0;
  const localeListeners = new Set<(payload: LocaleSnapshot) => void>();

  const publishLocale = (selection: LocaleSelection) => {
    localeGeneration += 1;
    const payload: LocaleSnapshot = {
      selection,
      effectiveLocale:
        selection === "zh-Hans" ? "zh-Hans" : selection === "en" ? "en" : "en",
      generation: localeGeneration,
      diagnostic: null,
    };
    localeListeners.forEach((listener) => listener(payload));
    return payload;
  };
  const detectedOverrides = new Set<string>();
  let preferences: AppPreferences = {
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: true,
    checkSkillUpdates: true,
  };
  const skills = fixture.skills.map((skill) => ({ ...skill }));
  const enabledSkillIds = new Map(
    fixture.agents.map((agent) => [agent.id, [...agent.enabledSkillIds]]),
  );
  const observedStates = new Map(
    fixture.agents.flatMap((agent) =>
      agent.enabledSkillIds.map(
        (skillId) =>
          [
            `${agent.id}:${skillId}`,
            agent.observedSkillStates?.[skillId] ?? "present",
          ] as const,
      ),
    ),
  );
  const plans = new Map<string, PlannedFixtureActivation>();
  const linkImportPlans = new Map<string, PlannedFixtureLinkImport>();
  const relocatePlans = new Map<
    string,
    { skillId: string; finalEntityPath: string }
  >();
  const removePlans = new Map<string, { skillId: string }>();
  let planCounter = 1;
  const replacePlans = new Map<string, PlannedFixtureReplace>();
  const appliedReplaces = new Map<string, PlannedFixtureReplace>();

  function discoverLink(sourcePath: string): LinkImportCandidate {
    const finalEntityPath = sourcePath.replace(/\/+$/, "");
    const directoryName = finalEntityPath.split("/").at(-1) ?? "";
    if (!directoryName) {
      throw { code: "validation", message: "Choose a Skill folder." };
    }
    return {
      directoryName,
      displayName: directoryName,
      description: "Linked local Skill.",
      frontmatterName: directoryName,
      sourceEntryPath: finalEntityPath,
      finalEntityPath,
    };
  }

  function activationFor(
    skillId: string,
    agent: FixtureAgent,
  ): AgentActivation {
    const desiredEnabled =
      enabledSkillIds.get(agent.id)?.includes(skillId) ?? false;
    return {
      id: agent.id,
      name: agent.name,
      kind: agent.kind,
      skillsPath: agent.skillsPath,
      detected: agent.detected,
      compatibility: agent.compatibility,
      desiredEnabled,
      observedState: desiredEnabled
        ? (observedStates.get(`${agent.id}:${skillId}`) ?? "missing")
        : "missing",
    };
  }

  function toDetail(skill: FixtureSkill): SkillDetail {
    const enabledAgentCount = fixture.agents.filter(
      (agent) => enabledSkillIds.get(agent.id)?.includes(skill.id) ?? false,
    ).length;
    return {
      ...skill,
      enabledAgentCount,
      fileSourceOriginalPath: skill.fileSourceOriginalPath ?? null,
    };
  }

  return {
    async prepareHome() {
      return fixtureUnsupported(
        "Home Binding is not available in the preview fixture",
      );
    },
    async confirmHome() {
      return fixtureUnsupported(
        "Home Binding is not available in the preview fixture",
      );
    },
    async continueCandidate() {
      return fixtureUnsupported(
        "Home Binding is not available in the preview fixture",
      );
    },
    async cancelCandidate() {
      return fixtureUnsupported(
        "Home Binding is not available in the preview fixture",
      );
    },
    async reconnectSameHome() {
      return fixtureUnsupported(
        "Reconnect is not available in the preview fixture",
      );
    },
    async planAbandon() {
      return fixtureUnsupported(
        "Abandon is not available in the preview fixture",
      );
    },
    async applyAbandon() {
      return fixtureUnsupported(
        "Abandon is not available in the preview fixture",
      );
    },
    async getRestoreEligibility() {
      return fixtureUnsupported(
        "Restore is not available in the preview fixture",
      );
    },
    async planRestore() {
      return fixtureUnsupported(
        "Restore is not available in the preview fixture",
      );
    },
    async getBootstrapSnapshot() {
      return {
        state: "bound",
        homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
        catalogAccess: "read_write",
        catalogReadonlyReason: null,
        snapshotVersion,
      };
    },
    async listenBootstrapChanged() {
      return () => {};
    },
    async getLocaleSnapshot() {
      return {
        selection: localeSelection,
        effectiveLocale: localeSelection === "zh-Hans" ? "zh-Hans" : "en",
        generation: localeGeneration,
        diagnostic: null,
      };
    },
    async setLocaleSelection(selection) {
      localeSelection = selection;
      return publishLocale(selection);
    },
    async refreshSystemLanguages() {
      return publishLocale("system");
    },
    async listenLocaleChanged(callback) {
      localeListeners.add(callback);
      return () => localeListeners.delete(callback);
    },
    async listSkills(filter) {
      return {
        snapshotVersion,
        items: skills
          .map(toDetail)
          .filter((skill) =>
            includesFilter(filter, skill.health, skill.sourceKind),
          )
          .map(toSummary),
      };
    },
    async inspectSkill(skillId) {
      const skill = skills.find(({ id }) => id === skillId);
      if (!skill) throw new Error(`Managed Skill '${skillId}' was not found`);
      return toDetail(skill);
    },
    async listAgents(skillId) {
      return fixture.agents.map((agent) => activationFor(skillId, agent));
    },
    async planActivation(skillId, agentId, enabled) {
      const skill = skills.find(({ id }) => id === skillId);
      const agent = fixture.agents.find(({ id }) => id === agentId);
      if (!skill || !agent) throw new Error("Managed Skill or Agent not found");
      const planToken = `fixture-activation-plan-${nextPlanId++}`;
      const preview: PlannedFixtureActivation = {
        planToken,
        skillId,
        agentId,
        skillDirectoryName: skill.directoryName,
        agentName: agent.name,
        enabled,
        kind: enabled ? "enable" : "disable",
        entryPath: `${agent.skillsPath}/${skill.directoryName}`,
        targetPath: skill.finalEntityPath,
        compatibilityWarning:
          agent.kind === "custom" ? { kind: "custom_unknown" } : null,
      };
      plans.set(planToken, preview);
      return preview;
    },
    async planActivationRepair(skillId, agentId) {
      const skill = skills.find(({ id }) => id === skillId);
      const agent = fixture.agents.find(({ id }) => id === agentId);
      if (!skill || !agent) throw new Error("Managed Skill or Agent not found");
      if (!enabledSkillIds.get(agent.id)?.includes(skillId)) {
        throw new Error("Repair requires a desired Activation");
      }
      const planToken = `fixture-activation-plan-${nextPlanId++}`;
      const preview: PlannedFixtureActivation = {
        planToken,
        skillId,
        agentId,
        skillDirectoryName: skill.directoryName,
        agentName: agent.name,
        enabled: true,
        kind: "repair",
        entryPath: `${agent.skillsPath}/${skill.directoryName}`,
        targetPath: skill.finalEntityPath,
        compatibilityWarning:
          agent.kind === "custom" ? { kind: "custom_unknown" } : null,
      };
      plans.set(planToken, preview);
      return preview;
    },
    async activationConflictDetails(skillId, agentId) {
      const skill = skills.find(({ id }) => id === skillId);
      const agent = fixture.agents.find(({ id }) => id === agentId);
      if (!skill || !agent) throw new Error("Managed Skill or Agent not found");
      return {
        skillId,
        agentId,
        entryPath: `${agent.skillsPath}/${skill.directoryName}`,
        targetPath: skill.finalEntityPath,
        occupier: {
          kind: "real_directory",
          symlinkTarget: null,
          finalEntityPath: `${agent.skillsPath}/${skill.directoryName}`,
          directoryName: skill.directoryName,
          isSkill: true,
          adoptable: true,
          notAdoptableReason: null,
        },
      };
    },
    async planActivationReplace(skillId, agentId) {
      const skill = skills.find(({ id }) => id === skillId);
      const agent = fixture.agents.find(({ id }) => id === agentId);
      if (!skill || !agent) throw new Error("Managed Skill or Agent not found");
      const planToken = `fixture-replace-plan-${nextPlanId++}`;
      const operationId = `fixture-replace-${nextPlanId}`;
      const preview: PlannedFixtureReplace = {
        planToken,
        operationId,
        skillId,
        agentId,
        skillDirectoryName: skill.directoryName,
        agentName: agent.name,
        entryPath: `${agent.skillsPath}/${skill.directoryName}`,
        targetPath: skill.finalEntityPath,
        backupPath: `${skill.directoryName}.backup`,
        occupantKind: "real_directory",
      };
      replacePlans.set(planToken, preview);
      return preview;
    },
    async applyActivationReplace(planToken) {
      const plan = replacePlans.get(planToken);
      if (!plan)
        throw { code: "plan_stale", message: "Replace preview expired." };
      replacePlans.delete(planToken);
      const current = enabledSkillIds.get(plan.agentId) ?? [];
      enabledSkillIds.set(
        plan.agentId,
        Array.from(new Set([...current, plan.skillId])),
      );
      observedStates.set(`${plan.agentId}:${plan.skillId}`, "present");
      appliedReplaces.set(plan.operationId, plan);
      snapshotVersion += 1;
      return {
        skillId: plan.skillId,
        agentId: plan.agentId,
        desiredEnabled: true,
        observedState: "present",
        snapshotVersion,
      };
    },
    async cancelActivationReplace(planToken) {
      return replacePlans.delete(planToken);
    },
    async undoActivationReplace(operationId) {
      const plan = appliedReplaces.get(operationId);
      if (!plan)
        throw { code: "plan_stale", message: "Replace already finalized." };
      appliedReplaces.delete(operationId);
      const current = enabledSkillIds.get(plan.agentId) ?? [];
      enabledSkillIds.set(
        plan.agentId,
        current.filter((skillId) => skillId !== plan.skillId),
      );
      observedStates.set(`${plan.agentId}:${plan.skillId}`, "occupied");
      snapshotVersion += 1;
      return {
        undone: true,
        error: null,
        snapshotVersion,
      };
    },
    async finalizeActivationReplace(operationId) {
      appliedReplaces.delete(operationId);
    },
    async loadPreferences() {
      return { ...preferences };
    },
    async updatePreferences(updates) {
      preferences = {
        ...preferences,
        ...(updates.launchAtLogin !== undefined
          ? { launchAtLogin: updates.launchAtLogin }
          : {}),
        ...(updates.showInDock !== undefined
          ? { showInDock: updates.showInDock }
          : {}),
        ...(updates.checkAppUpdates !== undefined
          ? { checkAppUpdates: updates.checkAppUpdates }
          : {}),
        ...(updates.checkSkillUpdates !== undefined
          ? { checkSkillUpdates: updates.checkSkillUpdates }
          : {}),
      };
      return { preferences: { ...preferences }, warning: null };
    },
    async checkAppUpdate() {
      return { status: "up_to_date" };
    },
    async downloadAppUpdate(updateId) {
      return { updateId, version: "0.2.0" };
    },
    async cancelAppUpdate(updateId) {
      return { updateId };
    },
    async installAppUpdate() {
      return undefined;
    },
    async startupInfo() {
      return {
        firstRun: !firstRunCompleted,
        agents: fixture.agents.map((agent) => ({
          id: agent.id,
          name: agent.name,
          kind: agent.kind,
          skillsPath: agent.skillsPath,
          detected: agent.detected || detectedOverrides.has(agent.id),
        })),
      };
    },
    async completeOnboarding() {
      firstRunCompleted = true;
    },
    async createAgentDirectory(agentId) {
      detectedOverrides.add(agentId);
      return {
        firstRun: !firstRunCompleted,
        agents: fixture.agents.map((agent) => ({
          id: agent.id,
          name: agent.name,
          kind: agent.kind,
          skillsPath: agent.skillsPath,
          detected: agent.detected || detectedOverrides.has(agent.id),
        })),
      };
    },
    async runActivationHealthCheck() {
      return {
        checked: Array.from(enabledSkillIds.values()).reduce(
          (count, skillIds) => count + skillIds.length,
          0,
        ),
        snapshotVersion,
      };
    },
    async relocateLink(skillId, sourcePath) {
      const skill = skills.find((item) => item.id === skillId);
      if (!skill) throw new Error("Skill not found");
      const directoryName = sourcePath.split("/").filter(Boolean).pop() ?? "";
      if (directoryName !== skill.directoryName) {
        throw new Error(
          `the relocated directory '${directoryName}' must keep the identity '${skill.directoryName}'`,
        );
      }
      const preview = {
        planToken: `relocate-plan-${planCounter++}`,
        skillId,
        directoryName: skill.directoryName,
        sourceEntryPath: sourcePath,
        finalEntityPath: sourcePath,
        displayName: skill.displayName,
        description: skill.description,
        frontmatterName: skill.frontmatterName ?? null,
        activationCount: Array.from(enabledSkillIds.values()).filter((ids) =>
          ids.includes(skillId),
        ).length,
      };
      relocatePlans.set(preview.planToken, {
        skillId,
        finalEntityPath: preview.finalEntityPath,
      });
      return preview;
    },
    async applyRelocateLink(planToken) {
      const plan = relocatePlans.get(planToken);
      if (!plan) throw new Error("Relocate preview expired");
      relocatePlans.delete(planToken);
      const skill = skills.find((item) => item.id === plan.skillId);
      if (!skill) throw new Error("Skill not found");
      skill.finalEntityPath = plan.finalEntityPath;
      skill.health = "healthy";
      snapshotVersion += 1;
      return {
        skillId: plan.skillId,
        directoryName: skill.directoryName,
        finalEntityPath: plan.finalEntityPath,
        activationCount: 0,
        snapshotVersion,
      };
    },
    async cancelRelocateLink() {
      return true;
    },
    async planRemoveSkill(skillId) {
      const skill = skills.find((item) => item.id === skillId);
      if (!skill) {
        throw { code: "not_found", message: "Managed Skill not found" };
      }
      const planToken = `remove-plan-${planCounter++}`;
      removePlans.set(planToken, { skillId });
      return {
        planToken,
        skillId,
        directoryName: skill.directoryName,
        sourceKind: skill.sourceKind,
        finalEntityPath: skill.finalEntityPath,
        activationCount: Array.from(enabledSkillIds.values()).filter((ids) =>
          ids.includes(skillId),
        ).length,
      };
    },
    async applyRemoveSkill(planToken) {
      const plan = removePlans.get(planToken);
      if (!plan) throw new Error("Remove preview expired");
      removePlans.delete(planToken);
      const index = skills.findIndex((item) => item.id === plan.skillId);
      if (index === -1) throw new Error("Skill not found");
      const [skill] = skills.splice(index, 1);
      for (const [agentId, ids] of enabledSkillIds) {
        enabledSkillIds.set(
          agentId,
          ids.filter((id) => id !== plan.skillId),
        );
      }
      snapshotVersion += 1;
      return {
        skillId: plan.skillId,
        directoryName: skill.directoryName,
        snapshotVersion,
      };
    },
    async cancelRemoveSkill(planToken) {
      return removePlans.delete(planToken);
    },
    async applyActivation(planToken) {
      const plan = plans.get(planToken);
      if (!plan) throw new Error("Activation preview expired");
      plans.delete(planToken);
      const current = enabledSkillIds.get(plan.agentId) ?? [];
      enabledSkillIds.set(
        plan.agentId,
        plan.enabled
          ? Array.from(new Set([...current, plan.skillId]))
          : current.filter((skillId) => skillId !== plan.skillId),
      );
      observedStates.set(
        `${plan.agentId}:${plan.skillId}`,
        plan.enabled ? "present" : "missing",
      );
      snapshotVersion += 1;
      return {
        skillId: plan.skillId,
        agentId: plan.agentId,
        desiredEnabled: plan.enabled,
        observedState: plan.enabled ? "present" : "missing",
        snapshotVersion,
      };
    },
    async cancelActivation(planToken) {
      return plans.delete(planToken);
    },
    async discoverLinkImport(sourcePath) {
      return discoverLink(sourcePath);
    },
    async planLinkImport(sourcePath) {
      const candidate = discoverLink(sourcePath);
      const existing = skills.find(
        ({ directoryName }) =>
          directoryName.toLocaleLowerCase() ===
          candidate.directoryName.toLocaleLowerCase(),
      );
      const planToken = `fixture-link-import-plan-${nextPlanId++}`;
      const preview: PlannedFixtureLinkImport = {
        ...candidate,
        planToken,
        skillId: `fixture-link-${nextPlanId}`,
        libraryEntryPath: null,
        conflict: existing
          ? {
              existingSkillId: existing.id,
              directoryName: existing.directoryName,
            }
          : null,
        canApply: !existing,
      };
      linkImportPlans.set(planToken, preview);
      return preview;
    },
    async applyLinkImport(planToken) {
      const plan = linkImportPlans.get(planToken);
      if (!plan) {
        throw { code: "plan_stale", message: "Import preview expired." };
      }
      linkImportPlans.delete(planToken);
      const existing = skills.find(
        ({ directoryName }) =>
          directoryName.toLocaleLowerCase() ===
          plan.directoryName.toLocaleLowerCase(),
      );
      if (existing) {
        throw {
          code: "conflict",
          message: `The Library already contains Managed Skill '${existing.directoryName}'.`,
        };
      }
      skills.unshift({
        id: plan.skillId,
        directoryName: plan.directoryName,
        displayName: plan.displayName,
        description: plan.description,
        sourceKind: "link",
        health: "healthy",
        finalEntityPath: plan.finalEntityPath,
        fileSourceOriginalPath: null,
        frontmatterName: plan.directoryName,
        lastActivityAt: "2026-08-03T00:00:00Z",
        skillMarkdown: `# ${plan.displayName}\n\nLinked local Skill.\n`,
      });
      snapshotVersion += 1;
      return {
        operationId: `fixture-${plan.skillId}`,
        skillId: plan.skillId,
        directoryName: plan.directoryName,
        finalEntityPath: plan.finalEntityPath,
        libraryEntryPath: null,
        snapshotVersion,
      };
    },
    async cancelLinkImport(planToken) {
      return linkImportPlans.delete(planToken);
    },
    async discoverGitImport() {
      return fixtureUnsupported(
        "Git Import is not available in the preview fixture",
      );
    },
    async planGitImportSelection() {
      return fixtureUnsupported(
        "Git Import is not available in the preview fixture",
      );
    },
    async applyGitImportSelection() {
      return fixtureUnsupported(
        "Git Import is not available in the preview fixture",
      );
    },
    async cancelGitImportSelection() {
      return false;
    },
    async checkSkillUpdates() {
      return { groups: [], errors: [], parentConflicts: [] };
    },
    async planSkillUpdates() {
      return fixtureUnsupported(
        "Skill Updates are not available in the preview fixture",
      );
    },
    async applySkillUpdates() {
      return fixtureUnsupported(
        "Skill Updates are not available in the preview fixture",
      );
    },
    async pinSkillUpdates() {
      return fixtureUnsupported(
        "Skill Updates are not available in the preview fixture",
      );
    },
    async scanAdopt() {
      return fixtureUnsupported(
        "Adopt is not available in the preview fixture",
      );
    },
    async planAdopt() {
      return fixtureUnsupported(
        "Adopt is not available in the preview fixture",
      );
    },
    async applyAdopt() {
      return fixtureUnsupported(
        "Adopt is not available in the preview fixture",
      );
    },
    async undoAdopt() {
      return fixtureUnsupported(
        "Adopt is not available in the preview fixture",
      );
    },
    async finalizeAdopt() {
      return fixtureUnsupported(
        "Adopt is not available in the preview fixture",
      );
    },
    async cancelAdopt() {
      return false;
    },
    async getFixtureRecoveryPreview() {
      return fixtureUnsupported(
        "Fixture Recovery is not available in the preview fixture",
      );
    },
    async planFixtureRecovery() {
      return fixtureUnsupported(
        "Fixture Recovery is not available in the preview fixture",
      );
    },
    async applyFixtureRecovery() {
      return fixtureUnsupported(
        "Fixture Recovery is not available in the preview fixture",
      );
    },
    async confirmFixtureRecoveryResult() {
      return fixtureUnsupported(
        "Fixture Recovery is not available in the preview fixture",
      );
    },
    async listSafetySnapshots() {
      return [];
    },
    async planDeleteSafetySnapshot() {
      return fixtureUnsupported(
        "Safety Snapshot deletion is not available in the preview fixture",
      );
    },
    async applyDeleteSafetySnapshot() {
      return fixtureUnsupported(
        "Safety Snapshot deletion is not available in the preview fixture",
      );
    },
  };
}

function fixtureUnsupported(message: string): never {
  throw { code: "fixture_unsupported", message };
}

function includesFilter(
  filter: CatalogFilter,
  health: Health,
  sourceKind: SourceKind,
) {
  if (filter === "all") return true;
  if (filter === "broken" || filter === "modified") return health === filter;
  if (filter === "link") return sourceKind === "link";
  return sourceKind === "remote_install" || sourceKind === "file_install";
}

function toSummary(skill: SkillDetail) {
  return {
    id: skill.id,
    directoryName: skill.directoryName,
    displayName: skill.displayName,
    description: skill.description,
    sourceKind: skill.sourceKind,
    health: skill.health,
    enabledAgentCount: skill.enabledAgentCount,
  };
}
