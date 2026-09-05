import type {
  ObservationAndScanSnapshot,
  ScanReportRow,
} from "../app/catalog-client";
import { createFixtureCatalogClient } from "./catalog";

type ReportSummary = NonNullable<
  ObservationAndScanSnapshot["currentReport"]["summary"]
>;

export const COMPLETE_SUMMARY: ReportSummary = {
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

export const LOCAL_CANDIDATE_ROW: ScanReportRow = {
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

/** Deterministic long-report preview; never connects to the native Catalog. */
export function createScanPreviewClient() {
  const client = createFixtureCatalogClient();
  const rows: ScanReportRow[] = Array.from({ length: 30 }, (_, index) => ({
    ...LOCAL_CANDIDATE_ROW,
    entityRef: `scan-report-v1:home:fixture-report-5:5:complete@5@${index + 1}`,
    entitySeq: index + 1,
    canonicalPath: `/dev/projects/team-shared-agent-skills/long-directory-for-layout-verification/skill-${index + 1}`,
    directoryNames: [`skill-${index + 1}`],
  }));
  client.publishScanReport(
    {
      ...COMPLETE_SUMMARY,
      counts: {
        ...COMPLETE_SUMMARY.counts,
        entries: 30,
        entities: 30,
        files: 90,
        bytes: 3000,
      },
      sourceCounts: { ...COMPLETE_SUMMARY.sourceCounts, localCandidates: 30 },
    },
    { local_candidates: rows },
  );
  return client;
}
