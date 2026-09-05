import { createFixtureCatalogClient } from "./catalog";

/** Development-only visual fixture: never fetches or installs a repository. */
export function createGitPreviewClient() {
  return createFixtureCatalogClient({
    gitSourceCapability: {
      sources: [
        {
          remoteId: "example-media",
          canonicalUrl: "https://github.com/example/media-skills",
          kind: "git_repository_source",
          members: [
            { skillId: "media-xray", skillPath: "media-xray", presence: true },
          ],
        },
      ],
    },
    gitPreview: {
      kind: "preview",
      preview: {
        provider: "github",
        sourceUrl: "https://github.com/example/skills",
        aliases: [],
        policy: {
          mode: "auto_release_tag_head",
          value: null,
          selectionKind: "head",
          selectedRef: "HEAD",
          resolvedCommit: "a".repeat(40),
        },
        externalOwnershipClaims: [],
        members: Array.from({ length: 12 }, (_, index) => ({
          directoryName: `example-skill-${index + 1}`,
          displayName: `示例技能 ${index + 1}`,
          description:
            "用于检查长列表、路径换行和预览窗口间距的示例内容。This fixture does not download or install any Skills.",
          skillPath: `agent-skills/web-design/example-skill-${index + 1}`,
          treeSummary: "b".repeat(40),
          action: "added",
        })),
      },
    },
  });
}
