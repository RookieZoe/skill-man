import fixtureJson from "../../fixtures/library-desk.json";

import type {
  ActivationPreview,
  AgentActivation,
  AgentKind,
  CatalogClient,
  CatalogFilter,
  Compatibility,
  Health,
  SkillDetail,
  SourceKind,
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

const fixture = fixtureJson as FixtureFile;

export function createFixtureCatalogClient(): CatalogClient {
  let snapshotVersion = fixture.snapshotVersion;
  let nextPlanId = 1;
  const enabledSkillIds = new Map(
    fixture.agents.map((agent) => [agent.id, [...agent.enabledSkillIds]]),
  );
  const plans = new Map<string, PlannedFixtureActivation>();

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
      observedState: desiredEnabled ? "present" : "missing",
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
        items: fixture.skills
          .map(toDetail)
          .filter((skill) =>
            includesFilter(filter, skill.health, skill.sourceKind),
          )
          .map(toSummary),
      };
    },
    async inspectSkill(skillId) {
      const skill = fixture.skills.find(({ id }) => id === skillId);
      if (!skill) throw new Error(`Managed Skill '${skillId}' was not found`);
      return toDetail(skill);
    },
    async listAgents(skillId) {
      return fixture.agents.map((agent) => activationFor(skillId, agent));
    },
    async planActivation(skillId, agentId, enabled) {
      const skill = fixture.skills.find(({ id }) => id === skillId);
      const agent = fixture.agents.find(({ id }) => id === agentId);
      if (!skill || !agent) throw new Error("Managed Skill or Agent not found");
      if (agent.kind !== "claude_preset") {
        throw new Error("This milestone only supports Claude Code");
      }
      const planToken = `fixture-activation-plan-${nextPlanId++}`;
      const preview: PlannedFixtureActivation = {
        planToken,
        skillId,
        agentId,
        skillDirectoryName: skill.directoryName,
        agentName: agent.name,
        enabled,
        entryPath: `${agent.skillsPath}/${skill.directoryName}`,
        targetPath: skill.finalEntityPath,
      };
      plans.set(planToken, preview);
      return preview;
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
  };
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
