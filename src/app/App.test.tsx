import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import type {
  ScanReportRow,
  SourceGroupPreviewOutcome,
} from "./catalog-client";
import { App } from "./App";

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
  expect(screen.getByText("SQLite pointer only")).toBeInTheDocument();
  expect(screen.getByText("Review imported instructions")).toBeInTheDocument();
  expect(screen.getByText(/can become instructions/)).toBeInTheDocument();

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
    await screen.findByRole("heading", { name: "linked-workflow" }),
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
  expect(screen.getByText(/complete Source Release/)).toBeInTheDocument();

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
  expect(screen.getByText("Not detected")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Skip setup" }));

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
  client.publishScanReport(COMPLETE_SUMMARY, {
    local_candidates: [LOCAL_CANDIDATE_ROW],
  });
  await screen.findByText("1 Untracked Skill found. Nothing changed yet.");
  expect(
    document.querySelector(".onboarding-untracked-list"),
  ).not.toBeInTheDocument();
  // Zero selection, zero adopt writes: Finish is the only completion action.
  await user.click(screen.getByRole("button", { name: "Finish" }));
  expect(
    screen.queryByRole("dialog", { name: "Welcome to Skill Man" }),
  ).not.toBeInTheDocument();
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
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(
    await screen.findByRole("progressbar", {
      name: "Scanning Agent and shared directories…",
    }),
  ).toBeInTheDocument();
  client.publishScanReport(COMPLETE_SUMMARY, {
    local_candidates: [LOCAL_CANDIDATE_ROW],
  });
  await screen.findByText("1 Untracked Skill found. Nothing changed yet.");
  // Skip stays available at every step: nothing was registered.
  await user.click(screen.getByRole("button", { name: "Skip setup" }));
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
  expect(dialog).toHaveTextContent("Exactly four switches");
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

test("shows manual app update failures in Preferences", async () => {
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

  expect(await screen.findByRole("alert")).toHaveTextContent(
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

test("onboarding creates a missing Agent directory with explicit confirmation", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
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
  client.createAgentDirectory = async () => ({
    firstRun: true,
    agents: [
      {
        id: "custom-workbench",
        name: "Custom Workbench",
        kind: "custom",
        skillsPath: "~/.custom-tools/skills",
        detected: true,
      },
    ],
  });
  render(<App client={client} />);

  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(await screen.findByText("Not detected")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Create directory" }));

  expect(await screen.findByText("Detected")).toBeInTheDocument();
  await waitFor(() => {
    expect(
      screen.queryByRole("button", { name: "Create directory" }),
    ).not.toBeInTheDocument();
  });
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
    await screen.findByText("The external Link entity is kept in place"),
  ).toBeInTheDocument();
  expect(screen.getByText("Activations to disable")).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Remove skill-authoring" }),
  );
  expect(
    await screen.findByRole("heading", {
      name: "skill-authoring left the Library",
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
  expect(
    screen.getByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
});
