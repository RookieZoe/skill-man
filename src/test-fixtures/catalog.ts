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
  EnableCell,
  EnablePlan,
  GlobalTargetGroup,
  Health,
  LinkImportCandidate,
  LinkImportPreview,
  LocaleSelection,
  LocaleSnapshot,
  ObservationAndScanSnapshot,
  RecentProjectFolder,
  PresetObservation,
  ScanReportRow,
  ScanReportSection,
  SkillDetail,
  SourceKind,
  SourceGroupPreviewOutcome,
  GitSourceCapabilityReport,
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
  observedSkillStates?: Record<string, string | null>;
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
  options: {
    emptyAgentConfigurations?: boolean;
    recentProjectFolders?: RecentProjectFolder[];
    gitPreview?: SourceGroupPreviewOutcome;
    gitSourceCapability?: GitSourceCapabilityReport;
  } = {},
): CatalogClient & FixtureReportPublish {
  let snapshotVersion = fixture.snapshotVersion;
  let nextPlanId = 1;
  let nextOperationId = 1;
  const pendingEnablePlans = new Map<
    string,
    {
      planToken: string;
      catalogGeneration: number;
      cells: EnableCell[];
    }
  >();
  const fixtureOperations = new Map<string, string>();
  const recentProjectFolders: RecentProjectFolder[] =
    options.recentProjectFolders ? [...options.recentProjectFolders] : [];
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
  const observedSkillStates = new Map(
    fixture.agents.map((agent) => [
      agent.id,
      new Map(Object.entries(agent.observedSkillStates ?? {})),
    ]),
  );
  const presetRows = [
    ["omp", "omp", "~/.omp/agent/skills", ".omp/skills"],
    ["claude-code", "Claude Code", "~/.claude/skills", ".claude/skills"],
    ["codex", "Codex", "~/.codex/skills", ".codex/skills"],
    ["gemini-cli", "Gemini CLI", "~/.gemini/skills", ".gemini/skills"],
    ["cursor", "Cursor", "~/.cursor/skills", ".cursor/skills"],
    ["opencode", "opencode", "~/.config/opencode/skills", ".opencode/skills"],
    ["github-copilot", "GitHub Copilot", "~/.copilot/skills", ".github/skills"],
    ["general", "General", "~/.agents/skills", ".agents/skills"],
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
          projectSkillsDir:
            agent.kind === "claude_preset"
              ? ".claude/skills"
              : agent.kind === "codex_preset"
                ? ".agents/skills"
                : null,
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
            includesFilter(
              filter,
              skill.health,
              skill.sourceKind,
              skill.enabledAgentCount,
            ),
          )
          .map(toSummary),
      };
    },
    async getGitSourceCapability() {
      return options.gitSourceCapability ?? { sources: [] };
    },
    async restoreCurrentSourceRelease() {
      throw new Error("restore is not available in the fixture client");
    },
    async createLocalSourceCopy() {
      throw new Error(
        "local source copy is not available in the fixture client",
      );
    },
    async removeGitSource() {
      throw new Error("source remove is not available in the fixture client");
    },
    async inspectSkill(skillId) {
      const skill = skills.find(({ id }) => id === skillId);
      if (!skill) throw new Error(`Managed Skill '${skillId}' was not found`);
      return toDetail(skill);
    },
    async getAgentManagementSnapshot() {
      return agentSnapshot();
    },
    async listTargetGroups(skillId) {
      const configs = options.emptyAgentConfigurations
        ? []
        : agentConfigurations;
      const groups: GlobalTargetGroup[] = [];
      for (const config of configs) {
        const target = config.roots.find(
          (root) => root.role === "activation_target",
        );
        if (!target) {
          continue;
        }
        const desired =
          enabledSkillIds.get(config.agentId)?.includes(skillId) ?? false;
        groups.push({
          targetRootId: target.rootId,
          configuredPath: target.configuredPath,
          consumers: [
            {
              agentId: config.agentId,
              agentName: config.name,
              compatibility: config.compatibility,
              userConfigured: config.origin === "custom",
            },
          ],
          availability: "available",
          diagnostic: null,
          desired,
          observedState:
            (observedSkillStates
              .get(config.agentId)
              ?.get(skillId) as GlobalTargetGroup["observedState"]) ?? null,
          action: "none",
        });
      }
      return {
        skillId,
        skillName:
          skills.find((skill) => skill.id === skillId)?.displayName ?? skillId,
        skillHealth:
          skills.find((skill) => skill.id === skillId)?.health ?? "healthy",
        agentGeneration: 1,
        groups,
      };
    },
    async planGlobalEnable(skillIds, targetGroupIds, cellResolutions) {
      const cellResolutionsMap = new Map(
        cellResolutions.map((entry) => [entry.cellKey, entry.resolution]),
      );
      const cells: EnableCell[] = targetGroupIds.flatMap((targetRootId) => {
        const config = agentConfigurations.find((agent) =>
          agent.roots.some(
            (root) =>
              root.role === "activation_target" && root.rootId === targetRootId,
          ),
        );
        const targetPath =
          config?.roots.find((root) => root.rootId === targetRootId)
            ?.configuredPath ?? "";
        return skillIds.map((skillId) => {
          const skill = skills.find((s) => s.id === skillId);
          const directoryName =
            skill?.directoryName ??
            (skillId.includes("::") ? skillId.split("::")[1] : skillId);
          const displayName =
            skill?.displayName ??
            (skillId.includes("::") ? skillId.split("::")[0] : skillId);
          const entryPath = targetPath ? `${targetPath}/${directoryName}` : "";
          const desired =
            enabledSkillIds.get(config?.agentId ?? "")?.includes(skillId) ??
            false;
          const cellKey = `${skillId}|${targetRootId}`;
          const resolution = cellResolutionsMap.get(cellKey) ?? "skip";
          return {
            cellKey,
            skillId,
            skillName: displayName,
            directoryName,
            directoryIdentityKey: skillId,
            targetRootId,
            targetPath,
            entryPath,
            finalEntityPath: skill?.finalEntityPath ?? "",
            action: "enable" as const,
            affectedAgentIds: config ? [config.agentId] : [],
            affectedAgentNames: config ? [config.name] : [],
            occupier: "empty" as const,
            occExactDirect: false,
            destructive: null,
            eligibility: desired ? ("no_op" as const) : ("ready" as const),
            blockedReason: null,
            resolution,
            detail: null,
            createSteps: [],
            hopEvidence: [],
          };
        });
      });

      // Intra-batch contention simulation:
      const entryGroups = new Map<string, number[]>();
      cells.forEach((cell, idx) => {
        const key = `${cell.targetRootId}|${cell.entryPath}`;
        const list = entryGroups.get(key) ?? [];
        list.push(idx);
        entryGroups.set(key, list);
      });
      for (const [, indices] of entryGroups) {
        if (indices.length > 1) {
          const winners = indices.filter(
            (i) =>
              cells[i].resolution === "replace" ||
              cells[i].resolution === "switch",
          );
          if (winners.length === 1) {
            const wIdx = winners[0];
            cells[wIdx].eligibility = "ready";
            for (const i of indices) {
              if (i !== wIdx) {
                cells[i].eligibility = "conflict";
                cells[i].resolution = "skip";
                cells[i].detail =
                  `contention with '${cells[wIdx].skillName}'; not selected as winner`;
              }
            }
          } else {
            for (const i of indices) {
              if (
                cells[i].eligibility !== "blocked" &&
                cells[i].eligibility !== "no_op"
              ) {
                cells[i].eligibility = "conflict";
                cells[i].resolution = "skip";
                cells[i].detail =
                  "multiple skills in this batch share this Directory Identity on this Target; choose a winner";
              }
            }
          }
        }
      }

      const plan: EnablePlan = {
        planToken: `fixture-enable-${nextPlanId++}`,
        scope: "global",
        writeGateGeneration: 0,
        catalogGeneration: snapshotVersion,
        agentGeneration: 1,
        cells,
      };
      pendingEnablePlans.set(plan.planToken, plan);
      return plan;
    },
    async planGlobalLifecycle(skillId, targetGroupId, action) {
      const config = agentConfigurations.find((agent) =>
        agent.roots.some(
          (root) =>
            root.role === "activation_target" && root.rootId === targetGroupId,
        ),
      );
      const desired =
        enabledSkillIds.get(config?.agentId ?? "")?.includes(skillId) ?? false;
      const observed =
        observedSkillStates.get(config?.agentId ?? "")?.get(skillId) ?? null;
      const eligibility: EnableCell["eligibility"] =
        action === "disable" && desired
          ? "ready"
          : action === "repair" && desired && observed !== "present"
            ? "ready"
            : action === "enable" && !desired
              ? "ready"
              : "no_op";
      const plan: EnablePlan = {
        planToken: `fixture-enable-${nextPlanId++}`,
        scope: "global",
        writeGateGeneration: 0,
        catalogGeneration: snapshotVersion,
        agentGeneration: 1,
        cells: [
          {
            cellKey: `${skillId}|${targetGroupId}`,
            skillId,
            skillName:
              skills.find((skill) => skill.id === skillId)?.displayName ??
              skillId,
            directoryName:
              skills.find((skill) => skill.id === skillId)?.directoryName ??
              skillId,
            directoryIdentityKey: skillId,
            targetRootId: targetGroupId,
            targetPath:
              config?.roots.find((root) => root.rootId === targetGroupId)
                ?.configuredPath ?? "",
            entryPath: "",
            finalEntityPath: "",
            action,
            affectedAgentIds: config ? [config.agentId] : [],
            affectedAgentNames: config ? [config.name] : [],
            occupier: "empty" as const,
            occExactDirect: false,
            destructive: null,
            eligibility,
            blockedReason: null,
            resolution: "skip" as const,
            detail: null,
            createSteps: [],
            hopEvidence: [],
          },
        ],
      };
      pendingEnablePlans.set(plan.planToken, plan);
      return plan;
    },
    async applyGlobalEnable(planToken) {
      const plan = pendingEnablePlans.get(planToken);
      if (!plan) {
        throw new Error("fixture enable plan not found");
      }
      pendingEnablePlans.delete(planToken);
      const operationId = `fixture-op-${nextOperationId++}`;
      const cells = plan.cells.map((cell) => {
        const config = agentConfigurations.find((agent) =>
          agent.roots.some(
            (root) =>
              root.role === "activation_target" &&
              root.rootId === cell.targetRootId,
          ),
        );
        if (!config) {
          return {
            cellKey: cell.cellKey,
            skillId: cell.skillId,
            targetRootId: cell.targetRootId,
            outcome: "failed" as const,
            diagnostic: "fixture target not found",
          };
        }
        if (cell.eligibility === "ready") {
          const base = (enabledSkillIds.get(config.agentId) ?? []).filter(
            (id) => id !== cell.skillId,
          );
          if (cell.action !== "disable") {
            base.push(cell.skillId);
          }
          enabledSkillIds.set(config.agentId, base);
          observedSkillStates
            .get(config.agentId)
            ?.set(cell.skillId, cell.action === "disable" ? null : "present");
          fixtureOperations.set(operationId, cell.cellKey);
          return {
            cellKey: cell.cellKey,
            skillId: cell.skillId,
            targetRootId: cell.targetRootId,
            outcome: "succeeded" as const,
            diagnostic: null,
          };
        }
        return {
          cellKey: cell.cellKey,
          skillId: cell.skillId,
          targetRootId: cell.targetRootId,
          outcome:
            cell.eligibility === "no_op"
              ? ("no_op" as const)
              : ("skipped" as const),
          diagnostic: null,
        };
      });
      publishObservation();
      return { operationId, cells, snapshotVersion };
    },
    async undoGlobalEnable(operationId) {
      const cellKey = fixtureOperations.get(operationId);
      fixtureOperations.delete(operationId);
      if (!cellKey) {
        publishObservation();
        return { operationId, cells: [], snapshotVersion };
      }
      const [skillId, targetRootId] = cellKey.split("|");
      const config = agentConfigurations.find((agent) =>
        agent.roots.some(
          (root) =>
            root.role === "activation_target" && root.rootId === targetRootId,
        ),
      );
      if (config) {
        const base = (enabledSkillIds.get(config.agentId) ?? []).filter(
          (id) => id !== skillId,
        );
        enabledSkillIds.set(config.agentId, base);
        observedSkillStates.get(config.agentId)?.set(skillId, null);
      }
      const result = {
        operationId,
        cells: [
          {
            cellKey,
            undone: true,
            diagnostic: null,
          },
        ],
        snapshotVersion,
      };
      publishObservation();
      return result;
    },
    async finalizeGlobalEnable() {
      return undefined;
    },
    async listRecentProjectFolders() {
      return [...recentProjectFolders];
    },
    async clearRecentProjectFolders() {
      recentProjectFolders.length = 0;
    },
    async planProjectEnable(
      skillIds,
      projectFolder,
      agentIds,
      cellResolutions,
    ) {
      const cellResolutionsMap = new Map(
        cellResolutions.map((entry) => [entry.cellKey, entry.resolution]),
      );
      const selectedConfigs = agentConfigurations.filter((agent) =>
        agentIds.includes(agent.agentId),
      );

      const groupsMap = new Map<string, typeof selectedConfigs>();
      for (const config of selectedConfigs) {
        if (!config.projectSkillsDir) continue;
        const dir = config.projectSkillsDir;
        const container = `${projectFolder}/${dir}`;
        const list = groupsMap.get(container) ?? [];
        list.push(config);
        groupsMap.set(container, list);
      }
      // Preview discloses every configured consumer of a resolved project
      // container, even when the user selected only one of those Agents.
      for (const config of agentConfigurations) {
        if (agentIds.includes(config.agentId) || !config.projectSkillsDir) {
          continue;
        }
        const container = `${projectFolder}/${config.projectSkillsDir}`;
        const consumers = groupsMap.get(container);
        if (
          consumers &&
          !consumers.some((item) => item.agentId === config.agentId)
        ) {
          consumers.push(config);
        }
      }

      const cells: EnableCell[] = [];
      for (const [container, configs] of groupsMap) {
        const targetRootId = `project:${container}`;
        for (const skillId of skillIds) {
          const cellKey = `${skillId}|${targetRootId}`;
          const resolution = cellResolutionsMap.get(cellKey) ?? "skip";
          const skill = skills.find((s) => s.id === skillId);
          const directoryName =
            skill?.directoryName ??
            (skillId.includes("::") ? skillId.split("::")[1] : skillId);
          const displayName =
            skill?.displayName ??
            (skillId.includes("::") ? skillId.split("::")[0] : skillId);
          cells.push({
            cellKey,
            skillId,
            skillName: displayName,
            directoryName,
            directoryIdentityKey: skillId,
            targetRootId,
            targetPath: container,
            entryPath: `${container}/${directoryName}`,
            finalEntityPath: skill?.finalEntityPath ?? "",
            action: "enable",
            affectedAgentIds: configs.map((c) => c.agentId),
            affectedAgentNames: configs.map((c) => c.name),
            occupier: "empty",
            occExactDirect: false,
            destructive: null,
            eligibility: "ready",
            blockedReason: null,
            resolution,
            detail: null,
            createSteps: [container],
            hopEvidence: configs.map((c) => ({
              agentId: c.agentId,
              agentName: c.name,
              configuredRelativePath: c.projectSkillsDir ?? ".skills",
              resolvedContainer: container,
              hops: [
                {
                  path: container,
                  kind: "directory" as const,
                  target: null,
                  device: 1,
                  inode: 1,
                },
              ],
            })),
          });
        }
      }

      // Intra-batch contention simulation:
      const entryGroups = new Map<string, number[]>();
      cells.forEach((cell, idx) => {
        const key = `${cell.targetRootId}|${cell.entryPath}`;
        const list = entryGroups.get(key) ?? [];
        list.push(idx);
        entryGroups.set(key, list);
      });
      for (const [, indices] of entryGroups) {
        if (indices.length > 1) {
          const winners = indices.filter(
            (i) => cells[i].resolution === "replace",
          );
          if (winners.length === 1) {
            const wIdx = winners[0];
            cells[wIdx].eligibility = "ready";
            for (const i of indices) {
              if (i !== wIdx) {
                cells[i].eligibility = "conflict";
                cells[i].resolution = "skip";
                cells[i].detail =
                  `contention with '${cells[wIdx].skillName}'; not selected as winner`;
              }
            }
          } else {
            for (const i of indices) {
              if (
                cells[i].eligibility !== "blocked" &&
                cells[i].eligibility !== "no_op"
              ) {
                cells[i].eligibility = "conflict";
                cells[i].resolution = "skip";
                cells[i].detail =
                  "multiple skills in this batch share this Directory Identity on this Target; choose a winner";
              }
            }
          }
        }
      }
      const plan: EnablePlan = {
        planToken: `fixture-project-enable-${nextPlanId++}`,
        scope: "project",
        writeGateGeneration: 0,
        catalogGeneration: snapshotVersion,
        agentGeneration: 1,
        projectRoot: {
          canonicalPath: projectFolder,
          identity: "1:1",
        },
        cells,
      };
      pendingEnablePlans.set(plan.planToken, plan);
      return plan;
    },
    async applyProjectEnable(planToken) {
      return this.applyGlobalEnable(planToken);
    },
    async undoProjectEnable(operationId) {
      return this.undoGlobalEnable(operationId);
    },
    async finalizeProjectEnable(operationId) {
      return this.finalizeGlobalEnable(operationId);
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
      const payload = observationSnapshot();
      observationListeners.forEach((listener) => listener(payload));
      return payload;
    },
    async refreshActivationHealth() {
      const payload = observationSnapshot();
      observationListeners.forEach((listener) => listener(payload));
      return payload;
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
    async ignoreScanLocalCandidate(identity, generation, entitySeq) {
      const summary = fixtureCurrentReport.summary;
      const row = fixtureReportPages.local_candidates.find(
        (item) =>
          item.kind === "source_verdict" && item.entitySeq === entitySeq,
      );
      if (
        !summary ||
        summary.contentIdentity !== identity ||
        summary.generation !== generation ||
        row?.kind !== "source_verdict" ||
        row.verdict !== "local"
      ) {
        throw new Error("stale candidate");
      }
      fixtureReportPages.local_candidates =
        fixtureReportPages.local_candidates.filter((item) => item !== row);
      fixtureReportPages.excluded.push({
        ...row,
        verdict: "excluded",
        reasonKind: "ignored",
        operations: [],
      });
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
      const target = agentSnapshot()
        .configurations.find((item) => item.agentId === agentId)
        ?.roots.find((root) => root.role === "activation_target");
      const activeIds = enabledSkillIds.get(agentId) ?? [];
      const lastReference = target?.consumerAgentIds.length === 1;
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
        blockingActivationSkillIds: lastReference ? [...activeIds] : [],
        retainedActivationCount: lastReference ? 0 : activeIds.length,
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
      if (options.gitPreview) return options.gitPreview;
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
    async undoSourceUpdate() {
      return fixtureUnsupported(
        "Source Update Undo is not available in the preview fixture",
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
  enabledAgentCount: number,
) {
  if (filter === "all") return true;
  if (filter === "broken" || filter === "modified") return health === filter;
  if (filter === "link") return sourceKind === "link";
  if (filter === "local") return sourceKind !== "remote_install";
  if (filter === "git") return sourceKind === "remote_install";
  if (filter === "enabled") return enabledAgentCount > 0;
  if (filter === "disabled") return enabledAgentCount === 0;
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
