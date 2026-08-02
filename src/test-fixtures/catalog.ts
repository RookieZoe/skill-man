import fixtureJson from "../../fixtures/library-desk.json";

import type {
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

const fixture = fixtureJson as FixtureFile;

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

function activationFor(skillId: string, agent: FixtureAgent): AgentActivation {
  const desiredEnabled = agent.enabledSkillIds.includes(skillId);
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
  const enabledAgentCount = fixture.agents.filter((agent) =>
    agent.enabledSkillIds.includes(skill.id),
  ).length;
  return { ...skill, enabledAgentCount };
}

const skills = fixture.skills.map(toDetail);

export const fixtureCatalogClient: CatalogClient = {
  async listSkills(filter) {
    return {
      snapshotVersion: fixture.snapshotVersion,
      items: skills
        .filter((skill) =>
          includesFilter(filter, skill.health, skill.sourceKind),
        )
        .map(toSummary),
    };
  },
  async inspectSkill(skillId) {
    const skill = skills.find(({ id }) => id === skillId);
    if (!skill) throw new Error(`Managed Skill '${skillId}' was not found`);
    return skill;
  },
  async listAgents(skillId) {
    return fixture.agents.map((agent) => activationFor(skillId, agent));
  },
};

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
