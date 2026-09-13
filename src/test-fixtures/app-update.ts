import { createFixtureCatalogClient } from "./catalog";

/** Long notes exercise the real update flow without downloading or installing. */
export function createAppUpdatePreviewClient() {
  const client = createFixtureCatalogClient();
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.1.3",
    currentVersion: "0.1.2",
    downloadSizeBytes: 9.7 * 1024 * 1024,
    updateId: "layout-preview",
    releaseNotes: [
      "# Skill Man v0.1.3",
      "修复应用更新弹窗的布局，发布说明支持 **Markdown**。",
      ...Array.from(
        { length: 6 },
        (_, i) =>
          `## ${i + 1}. 安装与更新\n\n- 先确认下载，验证完成后再确认安装。\n- 保留手动下载入口。\n\n[发布页面](https://github.com/RookieZoe/skill-man/releases/latest)\n\n\`\`\`sh\nshasum -a 256 -c SHA256SUMS\n\`\`\``,
      ),
    ].join("\n\n"),
  });
  client.downloadAppUpdate = async (updateId) => {
    await new Promise((resolve) => setTimeout(resolve, 4000));
    return { updateId, version: "0.1.3" };
  };
  return client;
}
