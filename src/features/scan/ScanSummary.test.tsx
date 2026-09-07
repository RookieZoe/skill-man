import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import type {
  ObservationAndScanSnapshot,
  ScanReportRow,
} from "../../app/catalog-client";
import {
  createFixtureCatalogClient,
  type FixtureReportPublish,
} from "../../test-fixtures/catalog";
import { ScanEvidenceLedger } from "./ScanEvidenceLedger";

type ReportSummary = NonNullable<
  ObservationAndScanSnapshot["currentReport"]["summary"]
>;

const COMPLETE_SUMMARY: ReportSummary = {
  generation: 4,
  runId: "fixture-report-4",
  contentIdentity: "scan-report-v1:home:fixture-report-4:4:complete",
  trigger: "manual" as const,
  state: "complete" as const,
  coverage: { completed: 1, failed: 0, unresponsive: 0 },
  counts: {
    roots: 1,
    entries: 3,
    entities: 3,
    files: 12,
    bytes: 4096,
    gitProbes: 1,
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
    gitGroups: 1,
    gitGroupsConflicted: 0,
    localCandidates: 2,
    conflictSets: 0,
    conflictMembers: 0,
    blocked: 0,
    deferred: 0,
    identityConflicts: 1,
    alreadyManaged: 1,
    excluded: 1,
    needsAttention: 1,
  },
};

const GIT_GROUP_ROW: ScanReportRow = {
  kind: "git_source_group",
  groupSeq: 1,
  provider: "github",
  canonicalRepository: "https://github.com/owner/example",
  repositoryRoot: "/home/agent-skills/repo",
  remoteUrlsSeen: ["https://github.com/owner/example"],
  memberEntitySeqs: [1],
  memberPaths: ["/home/agent-skills/repo"],
  memberNames: ["repo"],
  lockClaims: [],
  refs: [],
  lockPaths: [],
  status: "candidate",
  operations: [
    { operation: "git_fetch_and_manage", allowed: true, closedReason: null },
  ],
  detail: null,
};

const LOCAL_ROW: ScanReportRow = {
  kind: "source_verdict",
  entityRef: "scan-report-v1:home:fixture-report-4:4:complete@4@2",
  entitySeq: 2,
  verdict: "local",
  canonicalPath: "/dev/projects/my-skill",
  directoryNames: ["my-skill"],
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
  notes: ["git_metadata_not_used"],
  operations: [{ operation: "local_link", allowed: true, closedReason: null }],
};

const LOCAL_MOVE_ROW: ScanReportRow = {
  kind: "source_verdict",
  entityRef: "scan-report-v1:home:fixture-report-4:4:complete@4@3",
  entitySeq: 3,
  verdict: "local",
  canonicalPath: "/home/agent-skills/plain",
  directoryNames: ["plain"],
  appearances: 1,
  fileCount: 1,
  byteCount: 10,
  treeHash: "tree-sha256-v1:def",
  lockClaims: [],
  worktreeHints: [],
  reasonKind: null,
  detail: null,
  gitRefs: [],
  gitLockPaths: [],
  gitGroupSeq: null,
  conflictSetSeq: null,
  notes: [],
  operations: [
    {
      operation: "local_link_with_move",
      allowed: false,
      closedReason: "scan_incomplete",
    },
  ],
};

const ATTENTION_ROW: ScanReportRow = {
  kind: "source_verdict",
  entityRef: "scan-report-v1:home:fixture-report-4:4:complete@4@4",
  entitySeq: 4,
  verdict: "blocked",
  canonicalPath: "/home/agent-skills/broken",
  directoryNames: ["broken"],
  appearances: 1,
  fileCount: 0,
  byteCount: 0,
  treeHash: null,
  lockClaims: [],
  worktreeHints: [],
  reasonKind: "uninterpretable_metadata",
  detail: "the Git worktree metadata is not interpretable",
  gitRefs: [],
  gitLockPaths: [],
  gitGroupSeq: null,
  conflictSetSeq: null,
  notes: [],
  operations: [],
};

const EXCLUDED_ROW: ScanReportRow = {
  kind: "source_verdict",
  entityRef: "scan-report-v1:home:fixture-report-4:4:complete@4@5",
  entitySeq: 5,
  verdict: "already_managed",
  canonicalPath: "/dev/projects/managed",
  directoryNames: ["managed"],
  appearances: 1,
  fileCount: 2,
  byteCount: 20,
  treeHash: "tree-sha256-v1:fff",
  lockClaims: [],
  worktreeHints: [],
  reasonKind: null,
  detail: null,
  gitRefs: [],
  gitLockPaths: [],
  gitGroupSeq: null,
  conflictSetSeq: null,
  notes: [],
  operations: [],
};

const CONFLICT_SET_ROW: ScanReportRow = {
  kind: "conflict_set",
  setSeq: 1,
  directoryIdentityKey: "skill",
  directoryName: "skill",
  memberEntitySeqs: [1, 2],
  memberEntityRefs: [
    "scan-report-v1:home:fixture-report-4:4:complete@4@1",
    "scan-report-v1:home:fixture-report-4:4:complete@4@2",
  ],
  memberPaths: ["/dev/projects/skill-a", "/dev/projects/skill-b"],
  winnerEntitySeq: null,
};

const ROOT_COVERAGE_ROW: ScanReportRow = {
  kind: "root_coverage",
  index: 0,
  configuredPath: "/home/agent-skills",
  canonicalPath: "/home/agent-skills",
  state: "completed",
  consumerAgents: [{ agentId: "a1", agentName: "Claude" }],
  counts: {
    roots: 1,
    entries: 3,
    entities: 3,
    files: 12,
    bytes: 4096,
    gitProbes: 1,
    failedRoots: 0,
    configuredAgents: 1,
    declaredRoots: 1,
    canonicalRoots: 1,
  },
  elapsedMs: 500,
  slow: false,
  diagnostic: null,
};

function publish(
  client: ReturnType<typeof createFixtureCatalogClient> & FixtureReportPublish,
  summary: typeof COMPLETE_SUMMARY = COMPLETE_SUMMARY,
  pages: Partial<Record<string, ScanReportRow[]>> = {},
) {
  client.publishScanReport(
    summary,
    pages as Parameters<FixtureReportPublish["publishScanReport"]>[1],
  );
}

test("classified summary shows the funnel and the four count cards", async () => {
  const client = createFixtureCatalogClient();
  render(<ScanEvidenceLedger client={client} />);
  publish(client);
  await waitFor(() => {
    expect(screen.getByText("1 Agents")).toBeInTheDocument();
  });
  // §7.6: four cards count only; no selection control lives on the cards.
  const cards = screen.getByLabelText("Classification counts");
  for (const [label, value] of [
    ["Git source candidates", "1"],
    ["Local candidates", "2"],
    ["Conflict set", "0"],
    ["Excluded · already managed", "2"],
  ] as const) {
    const card = within(cards).getByLabelText(label);
    expect(card.textContent).toContain(value);
    expect(within(card).getByText(label)).toBeInTheDocument();
  }
  // Funnel labels are the five §7.6 layers.
  for (const text of [
    "1 declared roots",
    "1 canonical roots",
    "3 appearances",
    "3 entities",
  ]) {
    expect(screen.getAllByText(text).length).toBeGreaterThanOrEqual(1);
  }
});

test("candidate blocks render in the fixed §8.1 order with default-empty selections", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<ScanEvidenceLedger client={client} />);
  publish(client, COMPLETE_SUMMARY, {
    git_sources: [GIT_GROUP_ROW],
    local_candidates: [LOCAL_ROW],
    excluded: [EXCLUDED_ROW],
    needs_attention: [ATTENTION_ROW],
    roots: [ROOT_COVERAGE_ROW],
  });
  // Fixed block order: Needs attention → Git sources → Local sources →
  // Excluded/already Managed.
  await waitFor(() => {
    expect(screen.getByText("Needs attention")).toBeInTheDocument();
  });
  const blocks = screen.getAllByRole("heading", { level: 4 });
  const headingIndex = (name: string) =>
    blocks.findIndex((heading) => heading.textContent === name);
  const attentionIndex = headingIndex("Needs attention");
  const gitIndex = headingIndex("Git sources");
  const localIndex = headingIndex("Local sources");
  const excludedIndex = headingIndex("Excluded · already managed");
  expect(attentionIndex).toBeGreaterThanOrEqual(0);
  expect(attentionIndex).toBeLessThan(gitIndex);
  expect(gitIndex).toBeLessThan(localIndex);
  expect(localIndex).toBeLessThan(excludedIndex);
  // Local Link stays selectable; its checkbox starts unchecked (default empty).
  const localRegion = await screen.findByRole("region", {
    name: "Local sources",
  });
  const localLink = await within(localRegion).findByRole("checkbox", {
    name: "Local Link",
  });
  expect(localLink).not.toBeChecked();
  await user.click(localLink);
  expect(localLink).toBeChecked();
  // Blocked/Deferred candidates carry no selection control.
  const attentionRegion = screen.getByRole("region", {
    name: "Needs attention",
  });
  expect(
    within(attentionRegion).queryByRole("checkbox"),
  ).not.toBeInTheDocument();
});

test("incomplete report disables destructive eligibility with the Core closed reason", async () => {
  const client = createFixtureCatalogClient();
  render(<ScanEvidenceLedger client={client} />);
  publish(
    client,
    {
      ...COMPLETE_SUMMARY,
      state: "incomplete",
      incomplete: true,
      coverage: { completed: 1, failed: 1, unresponsive: 0 },
    },
    {
      local_candidates: [LOCAL_MOVE_ROW],
      roots: [
        {
          ...ROOT_COVERAGE_ROW,
          state: "failed",
          diagnostic: "Root identity changed before walking",
        },
      ],
    },
  );
  // Destructive move operation is shown blocked with the same closed reason
  // the Core reports; the failed Root diagnostic stays persistently visible.
  await screen.findByText(
    "Some directories could not be scanned. Replacement and migration are unavailable. Check the errors and rescan.",
  );
  const localRegion = await screen.findByRole("region", {
    name: "Local sources",
  });
  await within(localRegion).findByText(/blocked/);
  expect(
    screen.getAllByText(/Root identity changed before walking/).length,
  ).toBeGreaterThanOrEqual(1);
  const move = within(localRegion).queryByRole("checkbox", {
    name: /Move/,
  });
  expect(move).not.toBeInTheDocument();
});

test("Root coverage table lists consumer Agent, result and evidence summary", async () => {
  const client = createFixtureCatalogClient();
  render(<ScanEvidenceLedger client={client} />);
  publish(client, COMPLETE_SUMMARY, { roots: [ROOT_COVERAGE_ROW] });
  await waitFor(() => {
    expect(screen.getByRole("table")).toBeInTheDocument();
  });
  await waitFor(() => {
    expect(screen.getByText("Claude")).toBeInTheDocument();
  });
  expect(screen.getByText("completed")).toBeInTheDocument();
  expect(
    await screen.findByText("3 entries · 3 entities · 12 files · 4096 bytes"),
  ).toBeInTheDocument();
});

test("Conflict Set winner picker renders inside the Local sources block", async () => {
  const client = createFixtureCatalogClient();
  render(<ScanEvidenceLedger client={client} />);
  publish(
    client,
    {
      ...COMPLETE_SUMMARY,
      sourceCounts: {
        ...COMPLETE_SUMMARY.sourceCounts,
        conflictSets: 1,
        conflictMembers: 2,
        localCandidates: 0,
      },
    },
    {
      local_candidates: [],
      conflict_sets: [CONFLICT_SET_ROW],
    },
  );
  const localRegion = await screen.findByRole("region", {
    name: "Local sources",
  });
  // Default: no winner is selected; the winner radio is default-empty.
  const winnerRadios = (await within(localRegion).findAllByRole(
    "radio",
  )) as HTMLInputElement[];
  expect(winnerRadios).toHaveLength(2);
  expect(winnerRadios.every((radio) => !radio.checked)).toBe(true);
  expect(
    within(localRegion).getByText("No winner selected"),
  ).toBeInTheDocument();
  const cards = screen.getByLabelText("Classification counts");
  expect(within(cards).getByLabelText("Local candidates")).toHaveTextContent(
    "0",
  );
});
