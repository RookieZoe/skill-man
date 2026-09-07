import { createFixtureCatalogClient } from "./catalog";

/** Development-only visual fixture: never fetches or installs a repository. */
export function createGitPreviewClient(forceReplacement = false) {
  const client = createFixtureCatalogClient({
    gitSourceCapability: {
      sources: [
        {
          remoteId: "example-media",
          canonicalUrl: "https://github.com/example/media-skills",
          kind: "git_repository_source",
          selectedRef: "HEAD",
          resolvedCommit: "a".repeat(40),
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
        externalOwnershipClaims: forceReplacement
          ? [
              {
                lockPath: "/fixture/.agents/.skill-lock.json",
                entryName: "design",
                requestedRef: "HEAD",
              },
            ]
          : [],
        removedExternalClaims: forceReplacement ? ["design"] : [],
        addedMemberNames: forceReplacement ? ["ui"] : [],
        members: Array.from({ length: 12 }, (_, index) => ({
          directoryName:
            forceReplacement && index === 0
              ? "ui"
              : `example-skill-${index + 1}`,
          displayName:
            forceReplacement && index === 0 ? "ui" : `示例技能 ${index + 1}`,
          description:
            "用于检查长列表、路径换行和预览窗口间距的示例内容。This fixture does not download or install any Skills.",
          skillPath: `agent-skills/web-design/example-skill-${index + 1}`,
          treeSummary: "b".repeat(40),
          action: "added",
        })),
      },
    },
  });
  if (forceReplacement) {
    client.confirmSourceTransition = async () => ({
      operationId: "fixture-force",
      remoteId: "fixture-source",
      releaseId: "fixture-release",
      resolvedCommit: "a".repeat(40),
      memberCount: 12,
      snapshotVersion: 2,
      undoAvailable: false,
    });
    client.finalizeSourceTransition = async () => undefined;
  }
  return client;
}
