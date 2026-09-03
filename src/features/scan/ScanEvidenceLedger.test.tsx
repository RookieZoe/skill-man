import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import type {
  ObservationAndScanSnapshot,
  ScanReportRow,
} from "../../app/catalog-client";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { ScanEvidenceLedger } from "./ScanEvidenceLedger";

type ReportSummary = NonNullable<
  ObservationAndScanSnapshot["currentReport"]["summary"]
>;

const COMPLETE_SUMMARY: ReportSummary = {
  generation: 5,
  runId: "fixture-scan-manual-1",
  contentIdentity: "scan-report-v1:home:fixture-report-5:5:complete",
  trigger: "manual" as const,
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

test("never-scanned surface shows the honest state and offers Rescan", async () => {
  render(<ScanEvidenceLedger client={createFixtureCatalogClient()} />);
  expect(await screen.findByText("Never scanned")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Rescan" })).toBeInTheDocument();
});

test("Rescan action starts a single-flight run and shows real progress facts", async () => {
  const user = userEvent.setup();
  render(<ScanEvidenceLedger client={createFixtureCatalogClient()} />);
  await screen.findByText("Never scanned");
  await user.click(screen.getByRole("button", { name: "Rescan" }));
  await waitFor(() => {
    expect(screen.getByText("Scanning")).toBeInTheDocument();
  });
  expect(screen.getByText(/Walking roots/)).toBeInTheDocument();
  expect(screen.getByText(/manual/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  // Progress carries real facts, never a percent or an ETA.
  expect(screen.queryByText(/percent/i)).not.toBeInTheDocument();
  expect(screen.queryByText(/eta/i)).not.toBeInTheDocument();
});

test("cancel coordinates with the running Run", async () => {
  const user = userEvent.setup();
  render(<ScanEvidenceLedger client={createFixtureCatalogClient()} />);
  await screen.findByText("Never scanned");
  await user.click(screen.getByRole("button", { name: "Rescan" }));
  await screen.findByText("Scanning");
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => {
    expect(screen.getByText("Cancelled")).toBeInTheDocument();
  });
});

test("idle surfaces never expose write actions", async () => {
  render(<ScanEvidenceLedger client={createFixtureCatalogClient()} idle />);
  await screen.findByText("Never scanned");
  expect(
    screen.queryByRole("button", { name: "Rescan" }),
  ).not.toBeInTheDocument();
});

test("Local Link draft plans and applies; the batch stays undoable and finalizable", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const planned: Array<{ entityRef: string; action: string }> = [];
  let finalizedOperationId: string | null = null;
  client.publishScanReport(COMPLETE_SUMMARY, {
    local_candidates: [LOCAL_CANDIDATE_ROW],
  });
  client.planAdopt = async (reportGeneration, selections) => {
    planned.push(...selections.map((selection) => ({ ...selection })));
    return {
      planToken: "fixture-adopt-plan",
      reportGeneration,
      items: [],
      canApply: true,
    };
  };
  client.applyAdopt = async () => ({
    operationId: "fixture-adopt-operation",
    items: [
      {
        skillId: "prompt-linter",
        directoryName: "prompt-linter",
        adopted: true,
        error: null,
      },
    ],
    snapshotVersion: 8,
    undoAvailable: true,
  });
  client.undoAdopt = async (operationId) => ({
    operationId,
    items: [{ directoryName: "prompt-linter", undone: true, error: null }],
    snapshotVersion: 9,
  });
  client.finalizeAdopt = async (operationId) => {
    finalizedOperationId = operationId;
  };

  render(<ScanEvidenceLedger client={client} />);
  await screen.findByText("Complete");
  const include = await screen.findByRole("checkbox", { name: "Local Link" });
  expect(screen.getByRole("button", { name: "Plan Adopt" })).toBeDisabled();
  await user.click(include);
  await user.click(screen.getByRole("button", { name: "Plan Adopt" }));
  await screen.findByText("Planned 0 Skill(s)");
  expect(planned).toEqual([
    {
      entityRef: "scan-report-v1:home:fixture-report-5:5:complete@5@1",
      action: "local_link",
    },
  ]);
  await user.click(screen.getByRole("button", { name: "Apply" }));
  await screen.findByText("Adopted prompt-linter");
  await user.click(screen.getByRole("button", { name: "Finalize" }));
  expect(finalizedOperationId).toBe("fixture-adopt-operation");
  await screen.findByText("Adopt finalized; results can no longer be undone.");
});

test("typed eligibility refusal is rendered from the closed code", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.publishScanReport(COMPLETE_SUMMARY, {
    local_candidates: [LOCAL_CANDIDATE_ROW],
  });
  client.planAdopt = async () => {
    throw {
      error: {
        code: "adopt_eligibility",
        closedCode: "scan_coverage_incomplete",
        entitySeq: 1,
        detail: "a failed root left coverage incomplete",
      },
      diagnostic: { code: "command_error", message: "refused" },
    };
  };
  render(<ScanEvidenceLedger client={client} />);
  await screen.findByText("Complete");
  await user.click(await screen.findByRole("checkbox", { name: "Local Link" }));
  await user.click(screen.getByRole("button", { name: "Plan Adopt" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Cannot adopt: scan_coverage_incomplete — a failed root left coverage incomplete",
  );
});
