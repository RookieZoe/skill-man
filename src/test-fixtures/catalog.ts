import fixtureJson from "../../fixtures/library-desk.json";

import type {
  AgentConfiguration,
  AgentConfigurationDraft,
  AgentConfigurationPlan,
  AgentKind,
  AgentManagementSnapshot,
  AgentPreset,
  AppPreferences,
  CatalogClient,
  CatalogFilter,
  Compatibility,
  Health,
  LinkImportCandidate,
  LinkImportPreview,
  LocaleSelection,
  LocaleSnapshot,
  ObservationAndScanSnapshot,
  PresetObservation,
  ScanReportRow,
  ScanReportSection,
  SkillDetail,
  SourceKind,
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
}

interface FixtureFile {
  snapshotVersion: number;
  skills: FixtureSkill[];
  agents: FixtureAgent[];
}

interface PlannedFixtureLinkImport extends LinkImportPreview {
  skillId: string;
}

const fixture = fixtureJson as FixtureFile;

/** Test-only extras on the fixture client to publish a classified Report. */
export interface FixtureReportPublish {
  /** Publish a terminal classified Report; `pages` serve its sections. */
  publishScanReport(
    summary: NonNullable<
      ObservationAndScanSnapshot["currentReport"]["summary"]
    >,
    pages: Partial<Record<ScanReportSection, ScanReportRow[]>>,
  ): void;
}

export function createFixtureCatalogClient(
  options: { emptyAgentConfigurations?: boolean } = {},
): CatalogClient & FixtureReportPublish {
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
  let detectionGeneration = 0;
  const observationListeners = new Set<
    (payload: ObservationAndScanSnapshot) => void
  >();
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
  const presetRows = [
    ["omp", "omp", "~/.omp/agent/skills", ".omp/skills"],
    ["claude-code", "Claude Code", "~/.claude/skills", ".claude/skills"],
    ["codex", "Codex", "~/.agents/skills", ".agents/skills"],
    ["gemini-cli", "Gemini CLI", "~/.gemini/skills", ".gemini/skills"],
    ["cursor", "Cursor", "~/.cursor/skills", ".cursor/skills"],
    ["opencode", "opencode", "~/.config/opencode/skills", ".opencode/skills"],
    ["github-copilot", "GitHub Copilot", "~/.copilot/skills", ".github/skills"],
    ["zed", "Zed", "~/.agents/skills", ".agents/skills"],
    ["windsurf", "Windsurf", "~/.codeium/windsurf/skills", ".windsurf/skills"],
  ] as const;
  const agentPresets: AgentPreset[] = presetRows.map(
    ([presetKey, name, activationTarget, projectSkillsDir]) => ({
      presetKey,
      name,
      compatibility: "verified",
      roots: [activationTarget],
      activationTarget,
      projectSkillsDir,
    }),
  );
  let agentConfigurations: AgentConfiguration[] =
    options.emptyAgentConfigurations
      ? []
      : fixture.agents.map((agent) => ({
          agentId: agent.id,
          origin: agent.kind === "custom" ? "custom" : "preset",
          presetKey:
            agent.kind === "claude_preset"
              ? "claude-code"
              : agent.kind === "codex_preset"
                ? "codex"
                : null,
          name: agent.name,
          compatibility: agent.compatibility,
          projectSkillsDir: null,
          roots: [
            {
              rootId: `fixture-root:${agent.skillsPath.toLocaleLowerCase()}`,
              configuredPath: agent.skillsPath,
              pathIdentityKey: agent.skillsPath.toLocaleLowerCase(),
              role: "activation_target",
              consumerAgentIds: [agent.id],
              activationSkillIds: [...agent.enabledSkillIds],
            },
          ],
        }));
  const agentConfigurationPlans = new Map<
    string,
    {
      kind: AgentConfigurationPlan["kind"];
      agentId: string;
      draft: AgentConfigurationDraft | null;
    }
  >();
  const linkImportPlans = new Map<string, PlannedFixtureLinkImport>();
  const relocatePlans = new Map<
    string,
    { skillId: string; finalEntityPath: string }
  >();
  const removePlans = new Map<string, { skillId: string }>();
  let planCounter = 1;

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

  function agentSnapshot(): AgentManagementSnapshot {
    const consumers = new Map<string, string[]>();
    for (const configuration of agentConfigurations) {
      for (const root of configuration.roots) {
        const ids = consumers.get(root.pathIdentityKey) ?? [];
        ids.push(configuration.agentId);
        consumers.set(root.pathIdentityKey, ids);
      }
    }
    return {
      generation: snapshotVersion,
      configurations: agentConfigurations.map((configuration) => ({
        ...configuration,
        roots: configuration.roots.map((root) => ({
          ...root,
          consumerAgentIds: [
            ...(consumers.get(root.pathIdentityKey) ?? [configuration.agentId]),
          ],
        })),
      })),
      presets: agentPresets.map((preset) => ({
        ...preset,
        roots: [...preset.roots],
      })),
    };
  }

  function observationSnapshot(): ObservationAndScanSnapshot {
    const presetObservations: PresetObservation[] = agentPresets.map(
      (preset) => ({
        presetKey: preset.presetKey,
        name: preset.name,
        state: "present",
        roots: preset.roots.map((configuredPath) => ({
          configuredPath,
          state: "present" as const,
          canonicalPath: configuredPath,
          diagnostic: null,
        })),
      }),
    );
    return {
      homeId: null,
      writeGateGeneration: 0,
      agentConfigurationGeneration: snapshotVersion,
      detection: {
        generation: detectionGeneration,
        presetObservations,
        slow: false,
      },
      startupProbe: null,
      activationHealth: null,
      scanRun: fixtureScanRun,
      currentReport: fixtureCurrentReport,
    };
  }

  let fixtureScanRun: ObservationAndScanSnapshot["scanRun"] = null;
  let scanRunCounter = 0;
  const fixtureCurrentReport: ObservationAndScanSnapshot["currentReport"] = {
    summary: null,
    freshness: "stale",
    staleReasons: ["cross_startup"],
  };
  const scanRunListeners = new Set<
    (payload: ObservationAndScanSnapshot) => void
  >();
  // Test-only classified Report state: pages per section, served by
  // `getScanReportPage` when a summary is published.
  let fixtureReportPages: Record<ScanReportSection, ScanReportRow[]> = {
    roots: [],
    entities: [],
    appearances: [],
    diagnostics: [],
    git_sources: [],
    local_candidates: [],
    conflict_sets: [],
    needs_attention: [],
    excluded: [],
  };
  function publishObservation() {
    const payload = observationSnapshot();
    scanRunListeners.forEach((listener) => listener(payload));
  }

  function plannedConfiguration(
    agentId: string,
    draft: AgentConfigurationDraft,
  ): AgentConfiguration {
    const origin = draft.presetKey ? "preset" : "custom";
    return {
      agentId,
      origin,
      presetKey: draft.presetKey,
      name: draft.name.trim(),
      compatibility: draft.presetKey ? "verified" : "unknown",
      projectSkillsDir: draft.projectSkillsDir,
      roots: draft.roots.map((root) => ({
        rootId: `fixture-root:${root.configuredPath.toLocaleLowerCase()}`,
        configuredPath: root.configuredPath,
        pathIdentityKey: root.configuredPath.toLocaleLowerCase(),
        role: root.role,
        consumerAgentIds: [agentId],
        activationSkillIds: [],
      })),
    };
  }

  return {
    async prepareHome() {
      return fixtureUnsupported(
        "Home Binding is not available in the preview fixture",
      );
    },
    async prepareExistingHomeRecovery() {
      return fixtureUnsupported(
        "Existing Home Recovery is not available in the preview fixture",
      );
    },
    async cancelExistingHomeRecovery() {
      return fixtureUnsupported(
        "Existing Home Recovery is not available in the preview fixture",
      );
    },
    async confirmExistingHomeRecovery() {
      return fixtureUnsupported(
        "Existing Home Recovery is not available in the preview fixture",
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
    async getGitSourceCapability() {
      return { sources: [] };
    },
    async inspectSkill(skillId) {
      const skill = skills.find(({ id }) => id === skillId);
      if (!skill) throw new Error(`Managed Skill '${skillId}' was not found`);
      return toDetail(skill);
    },
    async getAgentManagementSnapshot() {
      return agentSnapshot();
    },
    async getObservationSnapshot() {
      return observationSnapshot();
    },
    async refreshDetection() {
      detectionGeneration += 1;
      const payload = observationSnapshot();
      observationListeners.forEach((listener) => listener(payload));
      return payload;
    },
    async refreshStartupProbe() {
      return observationSnapshot();
    },
    async refreshActivationHealth() {
      return observationSnapshot();
    },
    async getObservationPage(kind, generation) {
      return {
        generation,
        rows: [],
        nextOffset: null,
      };
    },
    listenObservationChanged(callback) {
      observationListeners.add(callback);
      scanRunListeners.add(callback);
      return Promise.resolve(() => {
        observationListeners.delete(callback);
        scanRunListeners.delete(callback);
      });
    },
    async startRescan(trigger) {
      fixtureScanRun = {
        runId: `fixture-scan-${trigger}-${scanRunCounter++}`,
        generation: 1,
        trigger,
        state: "running",
        phase: "walking",
        currentRoot: 0,
        counts: {
          roots: 1,
          entries: 0,
          entities: 0,
          files: 0,
          bytes: 0,
          gitProbes: 0,
          failedRoots: 0,
          configuredAgents: 0,
          declaredRoots: 1,
          canonicalRoots: 1,
        },
        roots: [
          {
            index: 0,
            configuredPath: "/tmp/root",
            canonicalPath: "/tmp/root",
            state: "walking",
            counts: {
              roots: 1,
              entries: 0,
              entities: 0,
              files: 0,
              bytes: 0,
              gitProbes: 0,
              failedRoots: 0,
              configuredAgents: 0,
              declaredRoots: 1,
              canonicalRoots: 1,
            },
            elapsedMs: 0,
            slow: false,
            diagnostic: null,
          },
        ],
        elapsedMs: 0,
        slow: false,
        diagnostic: null,
      };
      publishObservation();
      return observationSnapshot();
    },
    async cancelRescan(runId) {
      if (fixtureScanRun?.runId !== runId) {
        throw new Error(`no active Run with id ${runId}`);
      }
      fixtureScanRun = {
        ...fixtureScanRun,
        state: "cancelled",
        phase: "finalizing",
      };
      publishObservation();
      return observationSnapshot();
    },
    async getScanReportPage(cursor) {
      // Without a published Report the fixture serves honest empty pages;
      // with one it serves the section's rows from the fixture classification.
      const report = fixtureCurrentReport.summary;
      if (!report || report.contentIdentity !== cursor.reportContentIdentity) {
        return {
          reportContentIdentity: cursor.reportContentIdentity,
          runId: cursor.runId,
          generation: cursor.generation,
          section: cursor.section,
          rows: [],
          nextOffset: null,
        };
      }
      const rows = fixtureReportPages[cursor.section] ?? [];
      const start = cursor.offset;
      const pageRows = rows.slice(start, start + 64);
      const nextOffset =
        start + pageRows.length < rows.length ? start + pageRows.length : null;
      return {
        reportContentIdentity: cursor.reportContentIdentity,
        runId: cursor.runId,
        generation: cursor.generation,
        section: cursor.section,
        rows: pageRows,
        nextOffset,
      };
    },
    async planCreateAgentConfiguration(draft) {
      const planToken = `fixture-agent-plan-${planCounter++}`;
      const agentId = `fixture-agent-${planCounter}`;
      agentConfigurationPlans.set(planToken, {
        kind: "create",
        agentId,
        draft,
      });
      return {
        planToken,
        kind: "create",
        configuration: plannedConfiguration(agentId, draft),
        targetWillBeCreated: false,
        blockingActivationSkillIds: [],
        retainedActivationCount: 0,
      };
    },
    async planEditAgentConfiguration(agentId, draft) {
      const planToken = `fixture-agent-plan-${planCounter++}`;
      agentConfigurationPlans.set(planToken, {
        kind: "edit",
        agentId,
        draft,
      });
      return {
        planToken,
        kind: "edit",
        configuration: plannedConfiguration(agentId, draft),
        targetWillBeCreated: false,
        blockingActivationSkillIds: [],
        retainedActivationCount: 0,
      };
    },
    async planDeleteAgentConfiguration(agentId) {
      const planToken = `fixture-agent-plan-${planCounter++}`;
      agentConfigurationPlans.set(planToken, {
        kind: "delete",
        agentId,
        draft: null,
      });
      return {
        planToken,
        kind: "delete",
        configuration: null,
        targetWillBeCreated: false,
        blockingActivationSkillIds: [],
        retainedActivationCount:
          agentConfigurations.find(
            (configuration) => configuration.agentId === agentId,
          )?.roots[0]?.activationSkillIds.length ?? 0,
      };
    },
    async applyAgentConfigurationPlan(planToken) {
      const plan = agentConfigurationPlans.get(planToken);
      if (!plan) throw { code: "plan_stale" };
      if (plan.kind === "delete") {
        agentConfigurations = agentConfigurations.filter(
          (configuration) => configuration.agentId !== plan.agentId,
        );
      } else if (plan.draft) {
        const configuration = plannedConfiguration(plan.agentId, plan.draft);
        agentConfigurations =
          plan.kind === "create"
            ? [...agentConfigurations, configuration]
            : agentConfigurations.map((current) =>
                current.agentId === plan.agentId ? configuration : current,
              );
      }
      agentConfigurationPlans.delete(planToken);
      snapshotVersion += 1;
      return {
        agentId: plan.agentId,
        generation: snapshotVersion,
        deleted: plan.kind === "delete",
        affectedTargetRootIds: [],
      };
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
    async fetchLatestAndManage() {
      return fixtureUnsupported(
        "Installing Git Skills is not available in the preview fixture",
      );
    },
    async previewSourcePromotion() {
      return fixtureUnsupported(
        "Source Promotion is not available in the preview fixture",
      );
    },
    async confirmSourcePromotion() {
      return fixtureUnsupported(
        "Source Promotion is not available in the preview fixture",
      );
    },
    async finalizeSourcePromotion() {
      return fixtureUnsupported(
        "Source Promotion is not available in the preview fixture",
      );
    },
    async previewSourceUpdate() {
      return fixtureUnsupported(
        "Source Update is not available in the preview fixture",
      );
    },
    async confirmSourceUpdate() {
      return fixtureUnsupported(
        "Source Update is not available in the preview fixture",
      );
    },
    async finalizeSourceUpdate() {
      return fixtureUnsupported(
        "Source Update is not available in the preview fixture",
      );
    },
    async confirmSourceTransition() {
      return fixtureUnsupported(
        "Source Transition is not available in the preview fixture",
      );
    },
    async undoSourceTransition() {
      return fixtureUnsupported(
        "Source Undo is not available in the preview fixture",
      );
    },
    async finalizeSourceTransition() {
      return fixtureUnsupported(
        "Source Transition is not available in the preview fixture",
      );
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
    publishScanReport(summary, pages) {
      // Publishing a terminal Report closes any in-flight Run exactly like
      // production (a Run publishes atomically and becomes terminal).
      if (fixtureScanRun) {
        fixtureScanRun = { ...fixtureScanRun, state: "completed" };
      }
      fixtureCurrentReport.summary = summary;
      fixtureCurrentReport.freshness = "current";
      fixtureCurrentReport.staleReasons = [];
      fixtureReportPages = {
        roots: [],
        entities: [],
        appearances: [],
        diagnostics: [],
        git_sources: [],
        local_candidates: [],
        conflict_sets: [],
        needs_attention: [],
        excluded: [],
        ...pages,
      };
      publishObservation();
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
