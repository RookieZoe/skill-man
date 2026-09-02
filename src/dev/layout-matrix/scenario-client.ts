import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import type {
  CatalogClient,
  SkillDetail,
  SkillSummary,
} from "../../app/catalog-client";

export type MatrixContent = "normal" | "empty" | "error";
export type MatrixDensity = "standard" | "dense";
export type MatrixLanguage = "en" | "zh";
export type MatrixNotices = "none" | "lock" | "stack";

export interface MatrixScenario {
  content: MatrixContent;
  density: MatrixDensity;
  language: MatrixLanguage;
  notices: MatrixNotices;
}

export function scenarioFromParams(params: URLSearchParams): MatrixScenario {
  const contentParam = params.get("content");
  const noticesParam = params.get("notices");
  return {
    content:
      contentParam === "empty" || contentParam === "error"
        ? contentParam
        : "normal",
    density: params.get("density") === "dense" ? "dense" : "standard",
    language: params.get("language") === "zh" ? "zh" : "en",
    notices:
      noticesParam === "lock" || noticesParam === "stack"
        ? noticesParam
        : "none",
  };
}

export function scenarioKey(scenario: MatrixScenario) {
  return `${scenario.content}-${scenario.density}-${scenario.language}-${scenario.notices}`;
}

/** Force the tall three-step onboarding overlay for low-height scroll evidence. */
export function forceOnboarding(params: URLSearchParams) {
  return params.get("onboarding") === "1";
}

// Deliberately unbroken source-style strings (English and 简体中文) that force
// wrapping decisions; they are source fixture content, never translated copy.
const DENSE_EN =
  "UninterruptedEnglishTextWithoutAnyBreakingOpportunitiesThatForcesWrappingDecisionsInsideEveryPaneAtEveryBreakpointWidthInTheMatrix";
const DENSE_ZH =
  "不间断简体中文长文本用于验证换行行为在任何断点宽度下都不会产生页面级横向滚动或者内容被裁剪同时保持全部操作可达";
const DENSE_MARKDOWN_EN =
  "# Skill authoring\n\n" +
  DENSE_EN +
  "\n\n" +
  DENSE_EN +
  "\n\n## Workflow\n\n1. " +
  DENSE_EN +
  "\n2. " +
  DENSE_EN +
  "\n3. " +
  DENSE_EN +
  "\n";
const DENSE_MARKDOWN_ZH =
  "# Skill authoring\n\n" +
  DENSE_ZH +
  "\n\n" +
  DENSE_ZH +
  "\n\n## 工作流\n\n1. " +
  DENSE_ZH +
  "\n2. " +
  DENSE_ZH +
  "\n3. " +
  DENSE_ZH +
  "\n";

/**
 * Dev-only scenario client for the layout matrix. The production App is
 * mounted with explicit client injection (spec §4.7: browser tests and
 * prototypes must never fall back to fixture data automatically).
 */
export function createMatrixCatalogClient(
  scenario: MatrixScenario,
): CatalogClient {
  const client = createFixtureCatalogClient();
  const denseText = scenario.language === "zh" ? DENSE_ZH : DENSE_EN;
  const denseMarkdown =
    scenario.language === "zh" ? DENSE_MARKDOWN_ZH : DENSE_MARKDOWN_EN;

  if (scenario.notices !== "none") {
    client.getGitSourceCapability = async () => {
      throw {
        error: { code: "catalog_unavailable" },
        diagnostic: {
          code: "capability_failed",
          message: "Git source capability unavailable.",
        },
      };
    };
  }

  if (scenario.notices === "stack") {
    // Stacked Notices = Catalog read plus Source Capability failure.
    client.listSkills = async () => {
      throw new Error("Catalog read failed");
    };
  }

  if (forceOnboarding(new URLSearchParams(window.location.search))) {
    const realStartupInfo = client.startupInfo.bind(client);
    client.startupInfo = async () => ({
      ...(await realStartupInfo()),
      firstRun: true,
    });
  }

  if (scenario.content === "empty") {
    client.listSkills = async () => ({ snapshotVersion: 8, items: [] });
  } else if (scenario.content === "error") {
    client.listSkills = async () => {
      throw new Error("Catalog read failed");
    };
  } else if (scenario.density === "dense") {
    const realListSkills = client.listSkills.bind(client);
    const realInspectSkill = client.inspectSkill.bind(client);
    client.listSkills = async (filter) => {
      const snapshot = await realListSkills(filter);
      return {
        ...snapshot,
        items: snapshot.items.map((item) => applyDenseSummary(item, denseText)),
      };
    };
    client.inspectSkill = async (skillId) => {
      const detail = await realInspectSkill(skillId);
      return applyDenseDetail(detail, denseText, denseMarkdown);
    };
  }

  return client;
}

function applyDenseSummary(
  item: SkillSummary,
  denseText: string,
): SkillSummary {
  return { ...item, description: denseText };
}

function applyDenseDetail(
  detail: SkillDetail,
  denseText: string,
  denseMarkdown: string,
): SkillDetail {
  return {
    ...detail,
    description: denseText,
    finalEntityPath: `${detail.finalEntityPath}/${denseText.slice(0, 64)}`,
    skillMarkdown: denseMarkdown,
  };
}
