import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { createScanPreviewClient } from "../test-fixtures/scan-report";
import type {
  EnableCell,
  EnablePlan,
  ScanReportRow,
  SourceGroupPreviewOutcome,
} from "./catalog-client";
import { App } from "./App";
import { createGitPreviewClient } from "../test-fixtures/git-preview";

test.each([false, true])(
  "force replacement preserves its result when catalog refresh fails: %s",
  async (refreshFails) => {
    const user = userEvent.setup();
    const client = createFixtureCatalogClient({
      gitPreview: {
        kind: "preview",
        preview: {
          provider: "github",
          sourceUrl: "https://github.com/tw93/Waza",
          aliases: [],
          policy: {
            mode: "head",
            value: null,
            selectionKind: "head",
            selectedRef: "HEAD",
            resolvedCommit: "a".repeat(40),
          },
          members: [
            {
              directoryName: "ui",
              displayName: "ui",
              description: "New member",
              skillPath: "skills/ui",
              treeSummary: "b".repeat(40),
              action: "added",
            },
          ],
          externalOwnershipClaims: [
            {
              lockPath: "/fixture/.skill-lock.json",
              entryName: "design",
              requestedRef: "HEAD",
            },
          ],
          removedExternalClaims: ["design"],
          addedMemberNames: ["ui"],
        },
      },
    });
    client.confirmSourceTransition = vi.fn(async () => ({
      operationId: "force",
      remoteId: "waza",
      releaseId: "release",
      resolvedCommit: "a".repeat(40),
      memberCount: 1,
      snapshotVersion: 2,
      undoAvailable: true,
    }));
    render(<App client={client} />);
    await screen.findByRole("heading", { name: "skill-authoring" });
    await user.click(screen.getByRole("button", { name: "Import" }));
    await user.click(
      screen.getByRole("button", { name: "Install Git Skills" }),
    );
    await user.type(
      screen.getByRole("textbox", { name: "Repository URL" }),
      "tw93/Waza",
    );
    await user.click(
      screen.getByRole("button", { name: "Fetch latest preview" }),
    );
    const force = await screen.findByRole("button", {
      name: "Force remote replacement",
    });
    expect(force).toBeDisabled();
    expect(client.confirmSourceTransition).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("checkbox", { name: /remove the listed Skills/i }),
    );
    const previewDialog = screen.getByRole("dialog");
    previewDialog.scrollTop = 900;
    if (refreshFails)
      client.listSkills = async () => {
        throw new Error("refresh failed");
      };
    await user.click(force);
    expect(client.confirmSourceTransition).toHaveBeenCalledExactlyOnceWith({
      sourceType: "github",
      sourceUrl: "https://github.com/tw93/Waza",
      trackingPolicy: { mode: "head", value: null },
      expectedSelectedRef: "HEAD",
      expectedResolvedCommit: "a".repeat(40),
      expectedRemovedClaims: ["design"],
    });
    expect(
      await screen.findByText("Remote replacement complete"),
    ).toBeVisible();
    expect(screen.getByRole("dialog").scrollTop).toBe(0);
    if (refreshFails)
      expect(screen.getByRole("alert")).toHaveTextContent(/refresh/i);
    expect(screen.getByText("Removed: design")).toBeVisible();
    expect(
      screen.getByText(
        "Open the Library to review and manually enable new Skills.",
      ),
    ).toBeVisible();
  },
);

test("global activation changes refresh the Skill list count immediately", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();
  const row = screen.getByRole("button", { name: "skill-authoring" });
  expect(within(row).getByLabelText("2 Agents")).toBeInTheDocument();
  await user.click(await screen.findByRole("switch", { name: "Claude Code" }));
  await waitFor(() =>
    expect(within(row).getByLabelText("1 Agents")).toBeInTheDocument(),
  );
});

test.each([
  ["Local", "local"],
  ["Git", "git"],
  ["Enabled", "enabled"],
  ["Not enabled", "disabled"],
] as const)(
  "%s filter requests and displays the matching catalog",
  async (label, filter) => {
    const client = createFixtureCatalogClient();
    const list = vi.spyOn(client, "listSkills");
    render(<App client={client} />);
    await screen.findByRole("heading", { name: "skill-authoring" });
    await userEvent.click(screen.getByRole("button", { name: label }));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(filter));
    const expected = await client.listSkills(filter);
    expect(expected.items.length).toBeGreaterThan(0);
    await waitFor(() =>
      expect(
        screen.getByLabelText(`${expected.items.length} visible Skills`),
      ).toBeVisible(),
    );
  },
);

test("Git import refreshes members and repository grouping before returning to Library", async () => {
  const user = userEvent.setup();
  const client = createGitPreviewClient();
  const list = client.listSkills.bind(client);
  const capability = client.getGitSourceCapability.bind(client);
  let installed = false;
  client.listSkills = async (filter) => {
    const snapshot = await list(filter);
    return {
      ...snapshot,
      items: installed
        ? snapshot.items
        : snapshot.items.filter((s) => s.id !== "media-xray"),
    };
  };
  client.getGitSourceCapability = vi.fn(async () =>
    installed ? capability() : { sources: [] },
  );
  client.confirmSourceTransition = async () => {
    installed = true;
    return {
      operationId: "install",
      remoteId: "example-media",
      releaseId: "release",
      resolvedCommit: "a".repeat(40),
      memberCount: 1,
      snapshotVersion: 2,
      undoAvailable: false,
    };
  };
  client.finalizeSourceTransition = async () => undefined;
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.click(screen.getByRole("button", { name: "Install Git Skills" }));
  await user.type(
    screen.getByRole("textbox", { name: "Repository URL" }),
    "https://github.com/example/media-skills",
  );
  await user.click(
    screen.getByRole("button", { name: "Fetch latest preview" }),
  );
  await user.click(
    await screen.findByRole("button", { name: "Install all 12 Skills" }),
  );
  await user.click(await screen.findByRole("button", { name: "Close" }));
  const group = await screen.findByRole("button", {
    name: "example/media-skills 1",
  });
  await user.click(group);
  expect(
    screen.getByRole("button", { name: "media-xray" }),
  ).toBeInTheDocument();
  expect(client.getGitSourceCapability).toHaveBeenCalledTimes(2);
  await user.click(screen.getByRole("tab", { name: "Agents" }));
  await user.click(screen.getByRole("tab", { name: "Library" }));
  expect(
    screen.getByRole("button", { name: "example/media-skills 1" }),
  ).toBeInTheDocument();
});

test("scan Adopt and Undo refresh the Library without navigation", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const list = client.listSkills.bind(client);
  let adopted = false;
  client.listSkills = async (filter) => {
    const snapshot = await list(filter);
    return {
      ...snapshot,
      items: adopted
        ? snapshot.items
        : snapshot.items.filter((s) => s.id !== "media-xray"),
    };
  };
  client.publishScanReport(COMPLETE_SUMMARY, {
    local_candidates: [LOCAL_CANDIDATE_ROW],
  });
  client.planAdopt = async (reportGeneration) => ({
    planToken: "plan",
    reportGeneration,
    items: [],
    canApply: true,
  });
  client.applyAdopt = async () => {
    adopted = true;
    return {
      operationId: "adopt",
      items: [],
      snapshotVersion: 2,
      undoAvailable: true,
    };
  };
  client.undoAdopt = async (operationId) => {
    adopted = false;
    return { operationId, items: [], snapshotVersion: 3 };
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();
  await user.click(await screen.findByRole("button", { name: "Scan report" }));
  await user.click(await screen.findByRole("checkbox", { name: "Local Link" }));
  await user.click(screen.getByRole("button", { name: "Plan Adopt" }));
  await user.click(await screen.findByRole("button", { name: "Apply" }));
  await waitFor(() =>
    expect(
      document.querySelector('.library-source-heading[aria-label="Git 1"]'),
    ).not.toBeNull(),
  );
  await user.click(screen.getByRole("button", { name: "Undo" }));
  await waitFor(() =>
    expect(
      document.querySelector('.library-source-heading[aria-label="Git 1"]'),
    ).toBeNull(),
  );
});

test("repository management has its own third page and preserves the selected Skill", async () => {
  const user = userEvent.setup();
  render(<App client={createGitPreviewClient()} />);
  expect(
    await screen.findByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Update" }),
  ).not.toBeInTheDocument();
  expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
    "Library",
    "Agents",
    "Repositories",
  ]);
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  const repository = await screen.findByRole("heading", {
    name: "https://github.com/example/media-skills",
  });
  expect(repository.closest("article")).toBeInTheDocument();
  expect(
    screen.getByRole("navigation", { name: "Repositories" }),
  ).toBeVisible();
  expect(await screen.findByRole("button", { name: "Update" })).toBeVisible();
  expect(
    screen.getByRole("main", { name: "Repository management" }),
  ).toBeVisible();
  expect(
    screen.queryByRole("main", { name: "Skill detail" }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  expect(screen.getByRole("button", { name: "Update" })).toBeVisible();
  await user.click(screen.getByRole("tab", { name: "Library" }));
  expect(
    screen.getByRole("heading", { name: "skill-authoring" }),
  ).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "Update" }),
  ).not.toBeInTheDocument();
});

test("repository page shows an empty state without managed Git sources", async () => {
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await userEvent.click(screen.getByRole("tab", { name: "Repositories" }));
  expect(
    await screen.findByText(/No Git repositories managed yet/),
  ).toBeVisible();
});

test("repository selection shows only the chosen source details", async () => {
  const client = createGitPreviewClient();
  const report = await client.getGitSourceCapability();
  const second = {
    ...report.sources[0],
    remoteId: "second-repository",
    canonicalUrl: "https://github.com/tw93/kami",
  };
  client.getGitSourceCapability = async () => ({
    sources: [...report.sources, second],
  });
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await userEvent.click(screen.getByRole("tab", { name: "Repositories" }));
  const list = await screen.findByRole("navigation", { name: "Repositories" });
  await userEvent.click(
    within(list).getByRole("button", { name: /tw93\/kami/ }),
  );
  expect(
    await screen.findByRole("heading", { name: second.canonicalUrl }),
  ).toBeVisible();
  expect(
    screen.queryByRole("heading", { name: report.sources[0].canonicalUrl }),
  ).not.toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Update" })).toHaveLength(1);
});

test("repository member health is independent of the Library filter", async () => {
  const user = userEvent.setup();
  render(<App client={createGitPreviewClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Local" }));
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  await user.click(
    await screen.findByRole("heading", {
      name: "https://github.com/example/media-skills",
    }),
  );
  expect(
    await screen.findByText("Modified", { selector: ".member-health-badge" }),
  ).toBeVisible();
  await user.click(screen.getByRole("tab", { name: "Library" }));
  expect(screen.getByRole("button", { name: "Local" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  return {
    promise: new Promise<T>((next, fail) => {
      resolve = next;
      reject = fail;
    }),
    resolve,
    reject,
  };
}

test("scan workspace opens separately and returns without losing the selected Skill", async () => {
  const user = userEvent.setup();
  render(<App client={createScanPreviewClient()} />);
  await expandLibrary();
  await user.click(await screen.findByRole("button", { name: "media-xray" }));
  const open = screen.getByRole("button", { name: "Scan report" });
  expect(open).toHaveAttribute("aria-expanded", "false");
  const content = document.getElementById(open.getAttribute("aria-controls")!);
  expect(content).toHaveAttribute("hidden");
  await user.click(open);
  expect(content).not.toHaveAttribute("hidden");
  expect(
    screen.queryByRole("button", { name: "Batch actions" }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Back to workspace" }));
  expect(content).toHaveAttribute("hidden");
  expect(
    screen.getByRole("heading", { name: "media-xray" }),
  ).toBeInTheDocument();
});

test("technical evidence is collapsed and Agents keeps one working Rescan entry", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  const evidence = screen.getByText("Source and technical details", {
    selector: "summary",
  });
  expect(evidence.parentElement).not.toHaveAttribute("open");
  await user.click(evidence);
  expect(evidence.parentElement).toHaveAttribute("open");
  expect(
    screen.queryByRole("button", { name: "Health check" }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("tab", { name: "Agents" }));
  const rescans = screen.getAllByRole("button", { name: "Rescan" });
  expect(rescans).toHaveLength(1);
  expect(rescans[0]).toBeEnabled();
  await user.click(rescans[0]);
  expect(await screen.findByText("Scanning")).toBeInTheDocument();
});

test("opens the Library Desk with a selected Skill and Target-scoped placeholder", async () => {
  render(<App client={createFixtureCatalogClient()} />);

  expect(
    await screen.findByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("main", { name: "Skill detail" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("complementary", { name: "Activation Target Groups" }),
  ).toBeInTheDocument();
  expect(
    await screen.findByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();

  expect(
    await screen.findByRole("switch", { name: "Claude Code" }),
  ).toBeInTheDocument();
});

test("selects another Skill from the Library without leaving the three-column context", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await expandLibrary();

  const mediaXray = await screen.findByRole("button", { name: "media-xray" });
  await user.click(mediaXray);

  expect(
    await screen.findByRole("heading", { name: "media-xray" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Local changes detected")).toBeInTheDocument();
  expect(
    await screen.findByRole("switch", { name: "Codex" }),
  ).toBeInTheDocument();
});

test("filters the Library by health and selects the first remaining Skill", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Broken" }));

  expect(
    await screen.findByRole("heading", { name: "legacy-audit" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "media-xray" }),
  ).not.toBeInTheDocument();
  expect(screen.getByText("Source unavailable")).toBeInTheDocument();
});

test("imports a linked local folder and returns to its Library detail", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  const progress = screen.getByRole("list", { name: "Import progress" });
  expect(within(progress).getByText("Source")).toHaveAttribute(
    "aria-current",
    "step",
  );
  const source = "/Users/zoe/Codes/AI/skills/linked-workflow";
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    source,
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(
    await screen.findByRole("heading", { name: "Preview linked-workflow" }),
  ).toBeInTheDocument();
  expect(within(progress).getByText("Preview")).toHaveAttribute(
    "aria-current",
    "step",
  );
  expect(screen.getAllByText(source)).toHaveLength(2);
  expect(screen.getByText("Kept at the source location")).toBeInTheDocument();
  expect(screen.getByText("Review imported instructions")).toBeInTheDocument();
  expect(
    screen.getByText(/Make sure you trust the content/),
  ).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Import linked-workflow" }),
  );
  expect(
    await screen.findByRole("dialog", { name: "Link Import result" }),
  ).toHaveTextContent("linked-workflow is Managed");
  expect(within(progress).getByText("Result")).toHaveAttribute(
    "aria-current",
    "step",
  );
  await user.click(screen.getByRole("button", { name: "View in Library" }));

  expect(
    await screen.findByRole("heading", { name: "linked-workflow", level: 2 }),
  ).toBeInTheDocument();
  expect(
    await screen.findByRole("switch", { name: "Claude Code" }),
  ).toBeInTheDocument();
});

test("cancels Link discovery without creating a stale Preview", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let finishDiscovery:
    | ((
        candidate: Awaited<ReturnType<typeof client.discoverLinkImport>>,
      ) => void)
    | undefined;
  let planCalls = 0;
  client.discoverLinkImport = () =>
    new Promise((resolve) => {
      finishDiscovery = resolve;
    });
  const planLinkImport = client.planLinkImport.bind(client);
  client.planLinkImport = async (sourcePath) => {
    planCalls += 1;
    return planLinkImport(sourcePath);
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    "/tmp/cancellable-skill",
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(
    within(screen.getByRole("list", { name: "Import progress" })).getByText(
      "Discover",
    ),
  ).toHaveAttribute("aria-current", "step");
  const cancel = screen.getByRole("button", { name: "Cancel" });
  expect(cancel).toBeEnabled();
  await user.click(cancel);
  expect(screen.queryByRole("dialog", { name: "Import Link" })).toBeNull();

  await act(async () => {
    finishDiscovery?.({
      directoryName: "cancellable-skill",
      displayName: "cancellable-skill",
      description: "",
      frontmatterName: null,
      sourceEntryPath: "/tmp/cancellable-skill",
      finalEntityPath: "/tmp/cancellable-skill",
    });
  });
  await waitFor(() => expect(planCalls).toBe(0));
});

test("shows Library Conflict in Link preview and blocks Import", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    "/tmp/skill-authoring",
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Library Conflict",
  );
  expect(screen.getByText(/Rename the source/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Import skill-authoring" }),
  ).toBeDisabled();
  expect(
    screen.queryByRole("dialog", { name: "Link Import result" }),
  ).not.toBeInTheDocument();
});

test("local import opens a directory picker, preserves the path on cancel and still requires preview", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const preview = vi.spyOn(client, "discoverLinkImport");
  vi.mocked(open)
    .mockResolvedValueOnce("/Users/test/Skills/my-skill")
    .mockResolvedValueOnce(null);
  render(<App client={client} />);
  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.click(screen.getByRole("button", { name: "Choose folder…" }));
  expect(open).toHaveBeenCalledWith({ directory: true, multiple: false });
  expect(
    screen.getByRole("textbox", { name: "Local folder path" }),
  ).toHaveValue("/Users/test/Skills/my-skill");
  await user.click(screen.getByRole("button", { name: "Choose folder…" }));
  expect(
    screen.getByRole("textbox", { name: "Local folder path" }),
  ).toHaveValue("/Users/test/Skills/my-skill");
  expect(preview).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Preview Link" }));
  expect(preview).toHaveBeenCalledWith("/Users/test/Skills/my-skill");
});

test("Git import detects GitLab from its URL without a provider selector and rejects malformed input", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const fetch = vi.spyOn(client, "fetchLatestAndManage");
  render(<App client={client} />);
  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.click(screen.getByRole("button", { name: "Install Git Skills" }));
  expect(
    screen.queryByRole("combobox", { name: "Repository provider" }),
  ).not.toBeInTheDocument();
  const input = screen.getByRole("textbox", { name: "Repository URL" });
  await user.type(input, "not-a-url");
  expect(input).toHaveAttribute("aria-invalid", "true");
  expect(
    screen.getByRole("button", { name: "Fetch latest preview" }),
  ).toBeDisabled();
  expect(fetch).not.toHaveBeenCalled();
  await user.clear(input);
  await user.type(input, "https://gitlab.com/group/repo.git");
  await user.click(
    screen.getByRole("button", { name: "Fetch latest preview" }),
  );
  expect(fetch).toHaveBeenCalledWith(
    expect.objectContaining({
      sourceType: "gitlab",
      sourceUrl: "https://gitlab.com/group/repo.git",
    }),
  );
});

test("switches the Import sheet to Install Git Skills and reports a source rejection", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  const importDialog = screen.getByRole("dialog", { name: "Import Link" });
  expect(importDialog.children[1]).toHaveClass("import-kind-switch");
  await user.click(screen.getByRole("button", { name: "Install Git Skills" }));
  expect(
    screen.getByRole("dialog", { name: "Import from Git" }),
  ).toBeInTheDocument();
  expect(
    screen.getByText(/Preview does not change installed content/),
  ).toBeInTheDocument();

  await user.type(
    screen.getByRole("textbox", { name: "Repository URL" }),
    "https://github.com/owner/repo",
  );
  await user.click(
    screen.getByRole("button", { name: "Fetch latest preview" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "not available in the preview fixture",
  );
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(
    screen.queryByRole("dialog", { name: "Import from Git" }),
  ).not.toBeInTheDocument();
});

test("shows the three-step onboarding on first run and Skip records completion", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let completed = 0;
  client.startupInfo = async () => ({
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [
      {
        id: "claude-code",
        name: "Claude Code",
        kind: "claude_preset",
        skillsPath: "~/.claude/skills",
        detected: true,
      },
      {
        id: "codex",
        name: "Codex",
        kind: "codex_preset",
        skillsPath: "~/.codex/skills",
        detected: false,
      },
    ],
  });
  client.completeOnboarding = async () => {
    completed += 1;
  };
  render(<App client={client} />);

  const dialog = await screen.findByRole("dialog", {
    name: "Welcome to Skill Man",
  });
  expect(dialog).toHaveTextContent("Your Library");
  expect(dialog).toHaveTextContent("/Users/zoe/SkillMan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByText("Check Agent Presets")).toBeInTheDocument();
  expect(
    await screen.findByText("Already included in scan"),
  ).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Close" }));

  expect(completed).toBe(1);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(
    await screen.findByRole("main", { name: "Skill detail" }),
  ).toBeInTheDocument();
});

test("returns to the Library step when preset checking fails", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const info = {
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [],
  };
  let startupCalls = 0;
  client.startupInfo = () => {
    startupCalls += 1;
    return startupCalls === 1
      ? Promise.resolve(info)
      : Promise.reject(new Error("preset check failed"));
  };
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "Your Library" });
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "preset check failed",
  );
  expect(
    screen.getByRole("heading", { name: "Your Library" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "Check Agent Presets" }),
  ).not.toBeInTheDocument();
});

const COMPLETE_SUMMARY = {
  generation: 5,
  runId: "fixture-scan-manual-1",
  contentIdentity: "scan-report-v1:home:fixture-report-5:5:complete",
  trigger: "onboarding" as const,
  state: "complete" as const,
  coverage: { completed: 1, failed: 0, unresponsive: 0 },
  counts: {
    roots: 1,
    entries: 2,
    entities: 1,
    files: 4,
    bytes: 2048,
    gitProbes: 0,
    failedRoots: 0,
    configuredAgents: 1,
    declaredRoots: 1,
    canonicalRoots: 1,
  },
  incomplete: false,
  publishedAtMs: 1757000000000,
  agentConfigurationGeneration: 1,
  configuredRootSnapshotFingerprint: "roots<1>",
  startedAtMs: 1756999990000,
  slow: false,
  sourceCounts: {
    gitGroups: 0,
    gitGroupsConflicted: 0,
    localCandidates: 1,
    conflictSets: 0,
    conflictMembers: 0,
    blocked: 0,
    deferred: 0,
    identityConflicts: 0,
    alreadyManaged: 0,
    excluded: 0,
    needsAttention: 0,
  },
};

const LOCAL_CANDIDATE_ROW: ScanReportRow = {
  kind: "source_verdict",
  entityRef: "scan-report-v1:home:fixture-report-5:5:complete@5@1",
  entitySeq: 1,
  verdict: "local",
  canonicalPath: "/dev/projects/prompt-linter",
  directoryNames: ["prompt-linter"],
  appearances: 1,
  fileCount: 3,
  byteCount: 100,
  treeHash: "tree-sha256-v1:abc",
  lockClaims: [],
  worktreeHints: [],
  reasonKind: null,
  detail: null,
  gitRefs: [],
  gitLockPaths: [],
  gitGroupSeq: null,
  conflictSetSeq: null,
  notes: [],
  operations: [{ operation: "local_link", allowed: true, closedReason: null }],
};

const GIT_GROUP_ROW: ScanReportRow = {
  kind: "git_source_group",
  groupSeq: 1,
  provider: "github",
  canonicalRepository: "https://github.com/acme/skills",
  repositoryRoot: "/tmp/agent-skills/repo",
  remoteUrlsSeen: ["https://github.com/acme/skills"],
  memberEntitySeqs: [1],
  memberPaths: ["/tmp/agent-skills/repo/alpha"],
  memberNames: ["alpha"],
  lockClaims: [],
  refs: [],
  lockPaths: [],
  status: "candidate",
  operations: [
    { operation: "git_fetch_and_manage", allowed: true, closedReason: null },
  ],
  detail: null,
};

test("shows honest busy progress while checking presets and scanning untracked Skills", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const info = {
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [],
  };
  const checked = deferred<typeof info>();
  let startupCalls = 0;
  client.startupInfo = () => {
    startupCalls += 1;
    return startupCalls === 1 ? Promise.resolve(info) : checked.promise;
  };
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "Your Library" });
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(
    await screen.findByRole("progressbar", {
      name: "Checking Agent presets…",
    }),
  ).toHaveAttribute("aria-valuetext", "Checking Agent presets…");

  checked.resolve(info);
  await screen.findByRole("heading", { name: "Check Agent Presets" });
  await waitFor(() =>
    expect(
      screen.queryByRole("progressbar", {
        name: "Checking Agent presets…",
      }),
    ).not.toBeInTheDocument(),
  );

  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled(),
  );
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(
    await screen.findByRole("progressbar", {
      name: "Scanning Agent and shared directories…",
    }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("heading", { name: "Scan existing Skills" }),
  ).toBeInTheDocument();

  // The Run completes atomically and publishes the shared Report; the
  // onboarding scan step shows honest facts (counts only, nothing adopted).
  client.publishScanReport(
    {
      ...COMPLETE_SUMMARY,
      runId: "fixture-scan-onboarding-0",
      trigger: "onboarding",
    },
    {
      local_candidates: [LOCAL_CANDIDATE_ROW],
    },
  );
  await screen.findByText(/Scan complete: 1 Skill entity/);
  expect(
    document.querySelector(".onboarding-untracked-list"),
  ).not.toBeInTheDocument();
  // Zero selection, zero adopt writes: Finish is the only completion action.
  await user.click(screen.getByRole("button", { name: "Finish" }));
  expect(
    screen.queryByRole("dialog", { name: "Welcome to Skill Man" }),
  ).not.toBeInTheDocument();
});

test("a failed onboarding run cannot reuse an older successful report", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.startupInfo = async () => ({ firstRun: true, agents: [] });
  client.publishScanReport(COMPLETE_SUMMARY, {});
  const snapshot = client.getObservationSnapshot;
  client.getObservationSnapshot = async () => {
    const value = await snapshot();
    return {
      ...value,
      scanRun: value.scanRun
        ? { ...value.scanRun, state: "failed" as const }
        : null,
    };
  };
  render(<App client={client} />);
  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled(),
  );
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(
    await screen.findByText(
      "This scan did not complete. Retry the scan; no Skills were adopted.",
    ),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Finish" }),
  ).not.toBeInTheDocument();
});

test("scan scope setup can be reopened from Agent Management", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await user.click(screen.getByRole("tab", { name: "Agents" }));
  await user.click(
    screen.getByRole("button", { name: "Configure scan scope" }),
  );
  const dialog = await screen.findByRole("dialog", {
    name: "Welcome to Skill Man",
  });
  expect(
    await within(dialog).findByText("Already included in scan"),
  ).toBeInTheDocument();
});

test("onboarding scan allows zero selection zero registration and Skip", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.startupInfo = async () => ({
    firstRun: true,
    agents: [],
  });
  render(<App client={client} />);

  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled(),
  );
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(
    await screen.findByRole("progressbar", {
      name: "Scanning Agent and shared directories…",
    }),
  ).toBeInTheDocument();
  client.publishScanReport(
    {
      ...COMPLETE_SUMMARY,
      runId: "fixture-scan-onboarding-0",
      trigger: "onboarding",
    },
    {
      local_candidates: [LOCAL_CANDIDATE_ROW],
    },
  );
  await screen.findByText(/Scan complete: 1 Skill entity/);
  // Skip stays available at every step: nothing was registered.
  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(
    screen.queryByRole("dialog", { name: "Welcome to Skill Man" }),
  ).not.toBeInTheDocument();
});

test("scan summary Git source group hands the group to source management", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const remotePreview = deferred<SourceGroupPreviewOutcome>();
  const requests: Array<{
    sourceType: string;
    sourceUrl: string;
    trackingPolicy: { mode: string; value: string | null } | null;
  }> = [];
  client.fetchLatestAndManage = async (request) => {
    requests.push(request);
    return remotePreview.promise;
  };
  client.publishScanReport(COMPLETE_SUMMARY, {
    git_sources: [GIT_GROUP_ROW],
  });
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  // The group row offers the closed fetch-and-manage handoff — never a
  // per-member Include plan (spec §8.1).
  await user.click(await screen.findByRole("button", { name: "Scan report" }));
  const manage = await screen.findByRole("button", {
    name: "Fetch Latest and Manage",
  });
  expect(
    screen.queryByRole("checkbox", { name: /Local Link/ }),
  ).not.toBeInTheDocument();
  await user.click(manage);
  expect(
    await screen.findByRole("dialog", { name: "Import from Git" }),
  ).toBeInTheDocument();
  expect(requests).toEqual([
    {
      sourceType: "github",
      sourceUrl: "https://github.com/acme/skills",
      trackingPolicy: null,
    },
  ]);

  remotePreview.resolve({
    kind: "preview",
    preview: {
      provider: "github",
      sourceUrl: "https://github.com/acme/skills",
      aliases: [],
      policy: {
        mode: "auto_release_tag_head",
        value: null,
        selectionKind: "tag",
        selectedRef: "v1.0.0",
        resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
      },
      members: [
        {
          directoryName: "alpha",
          displayName: "Alpha",
          description: "The repository member",
          skillPath: "alpha",
          action: "added",
          treeSummary: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        },
      ],
      externalOwnershipClaims: [],
    },
  });
  expect(
    await screen.findByText("Complete Source Release"),
  ).toBeInTheDocument();
});

test("Preferences sheet shows exactly four switches with defaults", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));

  const dialog = await screen.findByRole("dialog", { name: "Preferences" });
  expect(within(dialog).getAllByRole("switch")).toHaveLength(4);
  const launch = screen.getByRole("switch", { name: "Launch at login" });
  expect(launch).not.toBeChecked();
  expect(screen.getByRole("switch", { name: "Show in Dock" })).toBeChecked();
  expect(
    screen.getByRole("switch", { name: "Check for app updates" }),
  ).toBeChecked();
  expect(
    screen.getByRole("switch", { name: "Check for Skill updates" }),
  ).toBeChecked();
  expect(dialog.querySelectorAll('input[type="checkbox"]')).toHaveLength(4);

  await user.click(launch);
  expect(launch).toBeChecked();
  await user.click(screen.getByRole("button", { name: "Done" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("downloads an available app update before asking to install and restart", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Adds signed, verified app updates.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-update-1",
  });
  let finishDownload:
    ((downloaded: { updateId: string; version: string }) => void) | undefined;
  client.downloadAppUpdate = () =>
    new Promise<{ updateId: string; version: string }>((resolve) => {
      finishDownload = resolve;
    });
  client.installAppUpdate = async (updateId) => {
    if (updateId !== "fixture-update-1") {
      throw new Error("The downloaded update was not installed.");
    }
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));

  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  expect(dialog).toHaveTextContent("0.2.0");
  expect(dialog).toHaveTextContent("Current version 0.1.0");
  expect(dialog).toHaveTextContent("Adds signed, verified app updates.");
  expect(dialog).toHaveTextContent("12 MB");

  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  expect(
    within(dialog).queryByRole("button", { name: "Install and Restart" }),
  ).not.toBeInTheDocument();

  await act(async () =>
    finishDownload?.({ updateId: "fixture-update-1", version: "0.2.0" }),
  );
  await user.click(
    await within(dialog).findByRole("button", {
      name: "Install and Restart",
    }),
  );
  await waitFor(() => {
    expect(
      screen.queryByRole("dialog", { name: "App update available" }),
    ).not.toBeInTheDocument();
  });
});

test("cancels an in-flight app update download without making it installable", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "A cancellable update.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-cancel-download",
  });
  let rejectDownload: ((reason: unknown) => void) | undefined;
  client.downloadAppUpdate = () =>
    new Promise((_, reject) => {
      rejectDownload = reject;
    });
  const cancelled: string[] = [];
  client.cancelAppUpdate = async (updateId) => {
    cancelled.push(updateId);
    rejectDownload?.({ code: "update_cancelled", message: "cancelled" });
    return { updateId };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  const cancelDownload = within(dialog).getByRole("button", {
    name: "Cancel download",
  });
  expect(cancelDownload).toHaveFocus();
  await user.click(cancelDownload);

  expect(cancelled).toEqual(["fixture-cancel-download"]);
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Install and Restart" }),
  ).not.toBeInTheDocument();
});

test("discards a downloaded app update when Later is chosen", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Discard after verification.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-discard-download",
  });
  client.downloadAppUpdate = async (updateId) => ({
    updateId,
    version: "0.2.0",
  });
  const cancelled: string[] = [];
  client.cancelAppUpdate = async (updateId) => {
    cancelled.push(updateId);
    return { updateId };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Later" }),
  );

  expect(cancelled).toEqual(["fixture-discard-download"]);
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
});

test("keeps a downloaded app update visible when discard fails", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Keep verified bytes until discard succeeds.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-discard-failure",
  });
  client.downloadAppUpdate = async (updateId) => ({
    updateId,
    version: "0.2.0",
  });
  client.cancelAppUpdate = async () => {
    throw {
      code: "state_unavailable",
      message: "Could not release the downloaded update.",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Later" }),
  );

  expect(dialog).toBeInTheDocument();
  expect(await within(dialog).findByRole("alert")).toHaveTextContent(
    "The update state could not be read. Restart Skill Man and retry.",
  );
  expect(
    within(dialog).getByRole("button", { name: "Install and Restart" }),
  ).toBeEnabled();
});

test("checks for an app update at startup when the preference is enabled", async () => {
  const client = createFixtureCatalogClient();
  client.checkAppUpdate = async (force) => {
    if (force) throw new Error("Expected the scheduled update check.");
    return {
      status: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      releaseNotes: "A scheduled update is ready.",
      downloadSizeBytes: 4 * 1024 * 1024,
      updateId: "fixture-scheduled-update",
    };
  };

  render(<App client={client} />);

  expect(
    await screen.findByRole("dialog", { name: "App update available" }),
  ).toHaveTextContent("A scheduled update is ready.");
});

test("keeps a failed offline startup update check silent", async () => {
  const client = createFixtureCatalogClient();
  let attempted = false;
  client.checkAppUpdate = async () => {
    attempted = true;
    throw new Error("The update server is offline.");
  };

  render(<App client={client} />);

  await waitFor(() => expect(attempted).toBe(true));
  expect(
    screen.queryByText(/update server is offline/i),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
});

test("shows manual app update failures only in background feedback", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => {
    throw {
      code: "source_unavailable",
      message: "The update server is offline.",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));

  expect(
    await screen.findByRole("region", { name: "Current activity" }),
  ).toHaveTextContent(
    "Unable to reach the update service. Check your network and retry.",
  );
  expect(
    screen.getByRole("dialog", { name: "Preferences" }),
  ).toBeInTheDocument();
});

test("closes the app update sheet with Escape and restores background focus", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "A focus-safe update.",
    downloadSizeBytes: 1024,
    updateId: "fixture-focus-update",
  });
  const { container } = render(<App client={client} />);

  const preferencesTrigger = await screen.findByRole("button", {
    name: "Preferences",
  });
  await user.click(preferencesTrigger);
  await user.click(screen.getByRole("button", { name: "Check now" }));

  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  const downloadButton = within(dialog).getByRole("button", {
    name: "Download update",
  });
  expect(downloadButton).toHaveFocus();
  await user.tab();
  expect(within(dialog).getByRole("button", { name: "Not now" })).toHaveFocus();
  await user.tab();
  expect(downloadButton).toHaveFocus();
  expect(container.querySelector(".app-background")).toHaveAttribute("inert");

  await user.keyboard("{Escape}");

  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
  expect(preferencesTrigger).toHaveFocus();
  expect(container.querySelector(".app-background")).not.toHaveAttribute(
    "inert",
  );
});

test("Preferences warning from the backend is shown inline", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.updatePreferences = async (updates) => ({
    preferences: {
      launchAtLogin: updates.launchAtLogin ?? false,
      showInDock: true,
      checkAppUpdates: true,
      checkSkillUpdates: true,
    },
    warning: {
      kind: "launch_at_login_failed",
      detail: "login item unavailable in dev build",
    },
  });
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await screen.findByRole("dialog", { name: "Preferences" });
  await user.click(screen.getByRole("switch", { name: "Launch at login" }));

  expect(await screen.findByText(/login item unavailable/)).toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Launch at login" })).toBeChecked();
});

test("onboarding saves a configuration only after reviewing and confirming its roots", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient({ emptyAgentConfigurations: true });
  client.startupInfo = async () => ({
    firstRun: true,
    agents: [
      {
        id: "custom-workbench",
        name: "Custom Workbench",
        kind: "custom",
        skillsPath: "~/.custom-tools/skills",
        detected: false,
      },
    ],
  });
  render(<App client={client} />);

  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));

  await user.click(
    await screen.findByRole("checkbox", { name: /Claude Code/ }),
  );
  expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  await user.click(
    screen.getByRole("button", { name: "Review selected configurations" }),
  );
  expect(
    (await client.getAgentManagementSnapshot()).configurations,
  ).toHaveLength(0);
  await user.click(
    await screen.findByRole("button", { name: "Confirm and save" }),
  );
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled(),
  );
  expect(
    (await client.getAgentManagementSnapshot()).configurations,
  ).toHaveLength(1);
});

test("Broken Link detail offers Relocate and restores health after preview confirm", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await user.click(screen.getByRole("button", { name: "Broken" }));
  await screen.findByRole("heading", { name: "legacy-audit" });
  expect(screen.getByText("Source unavailable")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Relocate…" }));
  expect(
    screen.getByRole("dialog", { name: "Relocate Broken Link" }),
  ).toBeInTheDocument();

  await user.type(
    screen.getByLabelText("New source path"),
    "~/Projects/moved/legacy-audit",
  );
  await user.click(screen.getByRole("button", { name: "Preview Relocate" }));

  expect(
    await screen.findByRole("heading", { name: "Relocate legacy-audit" }),
  ).toBeInTheDocument();
  expect(
    screen.getAllByText("~/Projects/moved/legacy-audit").length,
  ).toBeGreaterThan(0);
  expect(screen.getByText("Activations to update")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Relocate" }));
  expect(
    await screen.findByRole("heading", {
      name: "legacy-audit is healthy again",
    }),
  ).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.getByText("Healthy")).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Relocate…" }),
  ).not.toBeInTheDocument();
});

test("removes a Managed Skill after preview confirmation", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Remove…" }));

  expect(
    await screen.findByRole("dialog", { name: "Remove Managed Skill" }),
  ).toBeInTheDocument();
  expect(
    await screen.findByText("Source files stay in their original location"),
  ).toBeInTheDocument();
  expect(screen.getByText("Activations to disable")).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Remove skill-authoring" }),
  );
  expect(
    await screen.findByRole("heading", {
      name: "skill-authoring removed from Library",
    }),
  ).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(
    screen.queryByRole("heading", { name: "skill-authoring" }),
  ).not.toBeInTheDocument();
});

test("loads Git source capability states when the Catalog opens", async () => {
  const client = createFixtureCatalogClient();
  client.getGitSourceCapability = async () => ({
    sources: [
      {
        remoteId: "legacy-source",
        canonicalUrl: "https://github.com/acme/legacy",
        kind: "legacy_per_skill_git_state",
        members: [],
      },
    ],
  });

  render(<App client={client} />);

  await userEvent.click(screen.getByRole("tab", { name: "Repositories" }));
  expect(
    await screen.findByRole("region", { name: "Git source status" }),
  ).toHaveTextContent("Legacy Per-Skill Git State");
});

test("keeps a failed Git source scan visible without closing the Library", async () => {
  const client = createFixtureCatalogClient();
  client.getGitSourceCapability = async () => {
    throw {
      error: { code: "state_unavailable" },
      diagnostic: {
        code: "git_source_capability_scan_failed",
        message: "read-only Catalog could not be opened",
      },
    };
  };

  render(<App client={client} />);

  await userEvent.click(screen.getByRole("tab", { name: "Repositories" }));
  expect(
    await screen.findByRole("alert", { name: "Git source status" }),
  ).toHaveTextContent("Git source status unavailable");
  expect(
    screen
      .getByText(
        "git_source_capability_scan_failed: read-only Catalog could not be opened",
      )
      .closest("details"),
  ).not.toHaveAttribute("open");
  await userEvent.click(screen.getByRole("tab", { name: "Library" }));
  expect(
    screen.getByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
});

test("Source Promotion confirms with the draft provider and executes Undo", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const legacySource = {
    remoteId: "legacy-source",
    canonicalUrl: "https://gitlab.com/acme/legacy",
    kind: "legacy_per_skill_git_state" as const,
    members: [],
  };
  let confirmed:
    | {
        sourceType: string;
        sourceUrl: string;
      }
    | undefined;
  let undone: string | undefined;
  client.getGitSourceCapability = async () => ({
    sources: [legacySource],
  });
  client.previewSourcePromotion = async () => ({
    kind: "draft" as const,
    draft: {
      remoteId: "legacy-source",
      provider: "gitlab",
      sourceUrl: legacySource.canonicalUrl,
      aliases: [],
      policy: {
        mode: "branch",
        value: "main",
        selectionKind: "branch",
        selectedRef: "main",
        resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
      },
      members: [],
      removedMembers: [],
      legacyMemberCount: 0,
      externalOwnershipClaims: [],
    },
  });
  client.confirmSourcePromotion = async (request) => {
    confirmed = request;
    return {
      operationId: "promotion-operation-1",
      remoteId: request.remoteId,
      releaseId: "source-release-1",
      resolvedCommit: request.expectedResolvedCommit,
      memberCount: 0,
      snapshotVersion: 10,
      undoAvailable: true,
    };
  };
  client.undoSourceTransition = async (operationId) => {
    undone = operationId;
    return {
      operationId,
      memberCount: 0,
      snapshotVersion: 11,
    };
  };

  render(<App client={client} />);
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  await user.click(
    await screen.findByRole("button", { name: "Promote Legacy Source" }),
  );
  await screen.findByRole("heading", { name: "Promote Legacy Source" });
  await user.click(
    screen.getByRole("button", { name: "Promote complete source" }),
  );
  await screen.findByText("Whole Source Release is managed");
  expect(confirmed).toMatchObject({
    sourceType: "gitlab",
    sourceUrl: legacySource.canonicalUrl,
  });
  await user.click(screen.getByRole("button", { name: "Source Undo" }));
  await waitFor(() => expect(undone).toBe("promotion-operation-1"));
});

test("Source Update exposes an executable Undo window", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const source = {
    remoteId: "managed-source",
    canonicalUrl: "https://github.com/acme/managed",
    kind: "git_repository_source" as const,
    provider: "github",
    trackingMode: "auto_release_tag_head",
    trackingValue: null,
    selectedRef: "main",
    resolvedCommit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    members: [
      {
        skillId: "skill-authoring",
        skillPath: "skills/authoring",
        presence: true,
      },
    ],
  };
  let confirmed = false;
  let undone: string | undefined;
  client.getGitSourceCapability = async () => ({ sources: [source] });
  client.previewSourceUpdate = async () => ({
    remoteId: source.remoteId,
    provider: source.provider,
    sourceUrl: source.canonicalUrl,
    aliases: [],
    policy: {
      mode: "branch",
      value: "main",
      selectionKind: "branch",
      selectedRef: "main",
      resolvedCommit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    },
    members: [
      {
        skillId: "skill-authoring",
        skillPath: "skills/authoring",
        directoryName: "skill-authoring",
        directoryIdentityKey: "skill-authoring",
        displayName: "Skill authoring",
        description: "Updated",
        treeSummary: "cccccccccccccccccccccccccccccccccccccccc",
        state: "current",
      },
    ],
  });
  client.confirmSourceUpdate = async (request) => {
    confirmed = true;
    return {
      operationId: "update-operation-1",
      remoteId: request.remoteId,
      releaseId: "source-release-2",
      resolvedCommit: request.expectedResolvedCommit,
      memberCount: 1,
      snapshotVersion: 12,
      undoAvailable: true,
    };
  };
  client.undoSourceUpdate = async (operationId) => {
    undone = operationId;
    return { operationId, memberCount: 1, snapshotVersion: 13 };
  };

  render(<App client={client} />);
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  await user.click(
    await screen.findByRole("heading", {
      name: "https://github.com/acme/managed",
    }),
  );
  await user.click(await screen.findByRole("button", { name: "Update" }));
  await screen.findByRole("heading", {
    name: "Complete Source Release Update",
  });
  await user.click(
    screen.getByRole("button", { name: "Update complete source" }),
  );
  await screen.findByText("Whole Source Release is managed");
  expect(confirmed).toBe(true);
  expect(screen.getByRole("button", { name: "Source Undo" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Source Undo" }));
  await waitFor(() => expect(undone).toBe("update-operation-1"));
});

test("Global single-skill Adopt hands off to the ledger lifecycle", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const entityRef = "scan-report-v1:home:fixture-report-5:5:complete@5@1";
  const adoptCell = (targetRootId: string): EnableCell => ({
    cellKey: `skill-authoring|${targetRootId}`,
    skillId: "skill-authoring",
    skillName: "Skill authoring",
    directoryName: "prompt-linter",
    directoryIdentityKey: "prompt-linter",
    targetRootId,
    targetPath: "/Users/me/.claude/skills",
    entryPath: "/Users/me/.claude/skills/prompt-linter",
    finalEntityPath: "/Users/me/Library/skills/skill-authoring",
    action: "enable",
    affectedAgentIds: ["claude-code"],
    affectedAgentNames: ["Claude Code"],
    occupier: { untracked: { kind: "real_directory", target: null } },
    occExactDirect: false,
    destructive: { directories: 1, files: 0 },
    eligibility: "conflict",
    blockedReason: null,
    resolution: "skip",
    detail: null,
    createSteps: [],
    hopEvidence: [],
  });
  client.planGlobalEnable = async (_skillIds, targetRootIds) =>
    ({
      planToken: "global-adopt-handoff",
      scope: "global",
      writeGateGeneration: 0,
      catalogGeneration: 1,
      agentGeneration: 1,
      projectRoot: null,
      cells: [adoptCell(targetRootIds[0])],
    }) satisfies EnablePlan;
  let finalized = false;
  client.planAdopt = async (_generation, selections) => {
    expect(selections).toEqual([{ entityRef, action: "local_link" }]);
    return {
      planToken: "adopt-plan-1",
      reportGeneration: 5,
      items: [
        {
          entityRef,
          action: "local_link",
          directoryName: "prompt-linter",
          canonicalEntity: "/dev/projects/prompt-linter",
          finalEntityPath: "/dev/projects/prompt-linter",
          appearances: [],
          activations: [],
          applyable: true,
          error: null,
        },
      ],
      canApply: true,
    };
  };
  client.applyAdopt = async () => ({
    operationId: "adopt-operation-1",
    items: [
      {
        skillId: "adopted-prompt-linter",
        directoryName: "prompt-linter",
        adopted: true,
        error: null,
      },
    ],
    snapshotVersion: 2,
    undoAvailable: true,
  });
  client.finalizeAdopt = async () => {
    finalized = true;
  };
  client.publishScanReport(
    {
      ...COMPLETE_SUMMARY,
      sourceCounts: {
        ...COMPLETE_SUMMARY.sourceCounts,
        localCandidates: 1,
      },
    },
    {
      local_candidates: [
        {
          ...LOCAL_CANDIDATE_ROW,
          entityRef,
          directoryNames: ["prompt-linter"],
        },
      ],
      appearances: [
        {
          kind: "appearance",
          rootIndex: 0,
          seq: 1,
          name: "prompt-linter",
          entryPath: "/Users/me/.claude/skills/prompt-linter",
          entryKind: "directory",
          chain: [],
          chainFault: null,
          finalEntity: "/dev/projects/prompt-linter",
          identity: { device: 1, inode: 1 },
          entitySeq: 1,
          lockHint: null,
          worktreeHint: null,
        },
      ],
    },
  );

  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(
    await screen.findByRole("button", { name: "Enable globally…" }),
  );
  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  await user.click(within(dialog).getByText("Claude Code"));
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );
  await within(dialog).findByRole("combobox");
  await user.selectOptions(within(dialog).getByRole("combobox"), "adopt");
  await user.click(
    within(dialog).getByRole("button", { name: "Open Adopt surface" }),
  );

  expect(
    screen.queryByRole("dialog", {
      name: "Enable Skill authoring globally",
    }),
  ).not.toBeInTheDocument();
  expect(
    await screen.findByText("Selected prompt-linter in the Adopt surface."),
  ).toBeInTheDocument();
  const planAdopt = screen.getByRole("button", { name: "Plan Adopt" });
  expect(planAdopt).toBeEnabled();
  await user.click(planAdopt);
  await user.click(await screen.findByRole("button", { name: "Apply" }));
  await screen.findByText("Adopted prompt-linter");
  await user.click(screen.getByRole("button", { name: "Finalize" }));
  await waitFor(() => expect(finalized).toBe(true));
});

test("repository removal stays completed when refreshing the library fails", async () => {
  const user = userEvent.setup();
  const client = createGitPreviewClient();
  client.removeGitSource = vi.fn(async () => {
    client.listSkills = async () => {
      throw { code: "catalog_unavailable" };
    };
    return {
      operationId: "removed",
      remoteId: "example-media",
      memberCount: 1,
      snapshotVersion: 2,
    };
  });
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("tab", { name: "Repositories" }));
  await user.click(
    await screen.findByRole("heading", {
      name: "https://github.com/example/media-skills",
    }),
  );
  await user.click(screen.getByRole("button", { name: "Remove" }));
  await user.click(
    screen.getByRole("button", { name: "Remove complete source" }),
  );
  const notice = await screen.findByRole("region", {
    name: "Current activity",
  });
  expect(notice).toHaveTextContent("Source removed.");
  expect(notice.querySelector('[data-state="failed"]')).toBeNull();
  expect(client.removeGitSource).toHaveBeenCalledTimes(1);
});
import { expandLibrary } from "../test-fixtures/expand-library";
