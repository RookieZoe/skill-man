import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { RecoveryView } from "./RecoveryView";
import type {
  BootstrapSnapshot,
  CatalogClient,
  FixtureRecoveryPreview,
  SafetySnapshot,
} from "../../app/catalog-client";

const purePreview: FixtureRecoveryPreview = {
  mode: { kind: "legacy_unbound" },
  path: "/tmp/skill-man",
  classification: { kind: "pure" },
  catalogEvidence: {
    tables: ["activations", "agents", "catalog_meta"],
    schemaVersion: 4,
    firstRunCompletedAt: null,
    skillRowCount: 3,
    agentRowCount: 3,
    activationRowCount: 0,
    fileSourceRowCount: 0,
    remoteSourceRowCount: 0,
  },
  treeEvidence: {
    fixtureEntitiesPresent: true,
    skillAuthoringHashMatches: true,
    mediaXrayHashMatches: true,
    rootHashMatches: true,
    legacyAuditEntityPresent: false,
  },
  canPreview: true,
  activeOperation: null,
};

const mixedPreview: FixtureRecoveryPreview = {
  ...purePreview,
  classification: { kind: "mixed", reasons: ["skill_row_modified:media-xray"] },
  canPreview: false,
};

const snapshot: SafetySnapshot = {
  snapshotId: "skill-man.snapshot-fr-1-2",
  path: "/tmp/skill-man.snapshot-fr-1-2",
  manifestHash: "tree-sha256-v1:abc",
  fileCount: 42,
  totalBytes: 8192,
  takenAt: "2026-08-13T00:00:00Z",
};

function recoveryClient(overrides: Partial<CatalogClient> = {}): CatalogClient {
  return {
    getFixtureRecoveryPreview: vi.fn(async () => purePreview),
    listSafetySnapshots: vi.fn(async () => []),
    planFixtureRecovery: vi.fn(async () => ({ planToken: "op-1" })),
    applyFixtureRecovery: vi.fn(async () => ({
      operationId: "op-1",
      awaitingCommit: true,
      rolledBack: false,
    })),
    confirmFixtureRecoveryResult: vi.fn(async () => ({
      state: "legacy_detected",
      path: "/tmp/skill-man",
    })),
    planDeleteSafetySnapshot: vi.fn(async (snapshotId) => ({
      snapshotId,
      path: `/tmp/${snapshotId}`,
      fileCount: 42,
      totalBytes: 8192,
    })),
    applyDeleteSafetySnapshot: vi.fn(async () => {}),
    ...overrides,
  } as CatalogClient;
}

test("a pure preview offers the recovery action and shows the evidence", async () => {
  const client = recoveryClient();
  render(<RecoveryView client={client} />);

  expect(
    await screen.findByRole("heading", { name: "Fixture Recovery" }),
  ).toBeInTheDocument();
  expect(
    screen.getByText("Exact fixture footprint detected"),
  ).toBeInTheDocument();
  expect(screen.getByText("skill-authoring hash")).toBeInTheDocument();
  expect(
    screen.getAllByText("matches the fixture fingerprint"),
  ).toHaveLength(3);
  expect(
    screen.getByRole("button", { name: "Recover this Home" }),
  ).toBeInTheDocument();
});

test("the confirm-apply-commit flow drives the recovery operation", async () => {
  let activeOperation: FixtureRecoveryPreview["activeOperation"] = null;
  const client = recoveryClient({
    getFixtureRecoveryPreview: vi.fn(async () => ({
      ...purePreview,
      activeOperation,
    })),
    applyFixtureRecovery: vi.fn(async () => {
      activeOperation = {
        operationId: "op-1",
        cursor: "verified",
        snapshotPath: "/tmp/skill-man.snapshot-op-1",
        preparedPath: null,
      };
      return {
        operationId: "op-1",
        awaitingCommit: true,
        rolledBack: false,
      };
    }),
    confirmFixtureRecoveryResult: vi.fn(async (): Promise<BootstrapSnapshot> => {
      activeOperation = null;
      const snapshot: BootstrapSnapshot = {
        state: "legacy_detected",
        path: "/tmp/skill-man",
      };
      return snapshot;
    }),
  });
  const user = userEvent.setup();
  render(<RecoveryView client={client} />);

  await user.click(
    await screen.findByRole("button", { name: "Recover this Home" }),
  );
  expect(client.planFixtureRecovery).toHaveBeenCalledOnce();
  expect(client.applyFixtureRecovery).toHaveBeenCalledWith("op-1");
  expect(
    await screen.findByRole("button", { name: "Commit recovery result" }),
  ).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Commit recovery result" }),
  );
  expect(client.confirmFixtureRecoveryResult).toHaveBeenCalledWith("op-1");
});

test("an active awaiting-commit operation shows the commit action", async () => {
  const client = recoveryClient({
    getFixtureRecoveryPreview: vi.fn(async () => ({
      ...purePreview,
      activeOperation: {
        operationId: "op-7",
        cursor: "verified",
        snapshotPath: "/tmp/skill-man.snapshot-op-7",
        preparedPath: null,
      },
    })),
  });
  render(<RecoveryView client={client} />);

  expect(
    await screen.findByRole("button", { name: "Commit recovery result" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Recover this Home" }),
  ).not.toBeInTheDocument();
});

test("a mixed footprint locks the Home without recovery controls", async () => {
  const client = recoveryClient({
    getFixtureRecoveryPreview: vi.fn(async () => mixedPreview),
  });
  render(<RecoveryView client={client} />);

  expect(
    await screen.findByText("Modified fixture footprint detected"),
  ).toBeInTheDocument();
  expect(
    screen.getByText("skill_row_modified:media-xray"),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Recover this Home" }),
  ).not.toBeInTheDocument();
});

test("a rolled-back apply surfaces the failure and re-enables the flow", async () => {
  const client = recoveryClient({
    applyFixtureRecovery: vi.fn(async () => ({
      operationId: "op-1",
      awaitingCommit: false,
      rolledBack: true,
    })),
  });
  const user = userEvent.setup();
  render(<RecoveryView client={client} />);

  await user.click(
    await screen.findByRole("button", { name: "Recover this Home" }),
  );
  expect(
    await screen.findByRole("alert"),
  ).toHaveTextContent(/restored the original one/i);
  expect(
    screen.getByRole("button", { name: "Recover this Home" }),
  ).toBeInTheDocument();
});

test("snapshots list with explicit two-step deletion", async () => {
  const client = recoveryClient({
    listSafetySnapshots: vi.fn(async () => [snapshot]),
  });
  const user = userEvent.setup();
  render(<RecoveryView client={client} />);

  expect(
    await screen.findByText(snapshot.snapshotId),
  ).toBeInTheDocument();
  expect(screen.getByText(/42 files/)).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Delete…" }));
  expect(
    screen.getByRole("button", { name: "Delete permanently" }),
  ).toBeInTheDocument();
  await user.click(
    screen.getByRole("button", { name: "Delete permanently" }),
  );
  expect(client.planDeleteSafetySnapshot).toHaveBeenCalledWith(
    snapshot.snapshotId,
  );
  expect(client.applyDeleteSafetySnapshot).toHaveBeenCalledWith(
    snapshot.snapshotId,
  );
});
