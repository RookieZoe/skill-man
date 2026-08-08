import fixtureJson from "../../fixtures/library-desk.json";

import type {
  ActivationPreview,
  AgentActivation,
  AgentKind,
  CatalogClient,
  CatalogFilter,
  Compatibility,
  Health,
  LinkImportCandidate,
  LinkImportPreview,
  SkillDetail,
  SourceKind,
  ActivationObservedState,
} from "../app/catalog-client";

type FixtureSkill = Omit<SkillDetail, "enabledAgentCount">;

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

const fixture = fixtureJson as FixtureFile;

export function createFixtureCatalogClient(): CatalogClient {
  let snapshotVersion = fixture.snapshotVersion;
  let nextPlanId = 1;
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
    return { ...skill, enabledAgentCount };
  }

  return {
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
          agent.kind === "custom"
            ? "Custom Agent compatibility is unknown. Confirm this Activation explicitly."
            : null,
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
          agent.kind === "custom"
            ? "Custom Agent compatibility is unknown. Confirm this Activation explicitly."
            : null,
      };
      plans.set(planToken, preview);
      return preview;
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
        sourceLabel: `Linked local folder · ${plan.finalEntityPath}`,
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
      return fixtureUnsupported("Git Import is not available in the preview fixture");
    },
    async planGitImportSelection() {
      return fixtureUnsupported("Git Import is not available in the preview fixture");
    },
    async applyGitImportSelection() {
      return fixtureUnsupported("Git Import is not available in the preview fixture");
    },
    async cancelGitImportSelection() {
      return false;
    },
    async checkSkillUpdates() {
      return { groups: [], errors: [] };
    },
    async planSkillUpdates() {
      return fixtureUnsupported("Skill Updates are not available in the preview fixture");
    },
    async applySkillUpdates() {
      return fixtureUnsupported("Skill Updates are not available in the preview fixture");
    },
    async pinSkillUpdates() {
      return fixtureUnsupported("Skill Updates are not available in the preview fixture");
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
