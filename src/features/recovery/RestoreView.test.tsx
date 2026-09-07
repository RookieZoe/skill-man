import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { CatalogClient } from "../../app/catalog-client";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocaleProvider } from "../locale/LocaleProvider";
import { RestoreView } from "./RestoreView";

const REQUIRED = {
  kind: "restore_required" as const,
  homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
  path: "/tmp/skill-man-home",
  reason: "catalog_integrity_failed" as const,
};

const BOUND = {
  state: "bound" as const,
  homeId: REQUIRED.homeId,
  catalogAccess: "read_write" as const,
  catalogReadonlyReason: null,
  snapshotVersion: 1,
};

function renderView(
  eligibility: CatalogClient["getRestoreEligibility"] = async () => REQUIRED,
  overrides: Partial<CatalogClient> = {},
) {
  const client = createFixtureCatalogClient();
  Object.assign(client, {
    getRestoreEligibility: eligibility,
    listSafetySnapshots: vi.fn(async () => []),
    ...overrides,
  });
  const onSnapshot = vi.fn();
  render(
    <LocaleProvider client={client}>
      <RestoreView client={client} onSnapshot={onSnapshot} />
    </LocaleProvider>,
  );
  return { client, onSnapshot };
}

test("a healthy Home is not required and never offers Start", async () => {
  renderView(async () => ({ kind: "not_required" }));
  expect(await screen.findByText(/nothing to restore/)).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Start Restore" }),
  ).not.toBeInTheDocument();
});

test("a not-applicable reason is shown closed", async () => {
  renderView(async () => ({
    kind: "not_applicable",
    reason: "identity_mismatch",
  }));
  expect(
    await screen.findByText(/does not match the original Home/),
  ).toBeInTheDocument();
});

test("the restore flow plans, applies and commits with the same home_id facts", async () => {
  const plan = vi.fn(async () => ({ planToken: "fr-op-1" }));
  const apply = vi.fn(async () => ({
    operationId: "fr-op-1",
    awaitingCommit: true,
    rolledBack: false,
  }));
  const confirm = vi.fn(async () => BOUND);
  const { onSnapshot } = renderView(undefined, {
    planRestore: plan,
    applyFixtureRecovery: apply,
    confirmFixtureRecoveryResult: confirm,
  });

  expect(
    await screen.findByRole("heading", { name: "Restore Bound Home" }),
  ).toBeInTheDocument();
  expect(
    await screen.findByText(
      /Catalog integrity or foreign-key verification failed/,
    ),
  ).toBeInTheDocument();

  await userEvent.click(screen.getByRole("button", { name: "Start Restore" }));
  await waitFor(() => expect(plan).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(apply).toHaveBeenCalledWith("fr-op-1"));
  expect(await screen.findByText(/The Home is ready/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Commit Restore Result" }),
  ).toBeInTheDocument();

  await userEvent.click(
    screen.getByRole("button", { name: "Commit Restore Result" }),
  );
  await waitFor(() => expect(confirm).toHaveBeenCalledWith("fr-op-1"));
  await waitFor(() =>
    expect(onSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({ state: "bound" }),
    ),
  );
});

test("a rolled-back apply reports it and offers Start again", async () => {
  const plan = vi.fn(async () => ({ planToken: "fr-op-2" }));
  const apply = vi.fn(async () => ({
    operationId: "fr-op-2",
    awaitingCommit: false,
    rolledBack: true,
  }));
  renderView(undefined, {
    planRestore: plan,
    applyFixtureRecovery: apply,
  });

  await userEvent.click(
    await screen.findByRole("button", { name: "Start Restore" }),
  );
  expect(
    await screen.findByText(/original Home was restored/),
  ).toBeInTheDocument();
  expect(
    await screen.findByRole("button", { name: "Start Restore" }),
  ).toBeInTheDocument();
});

test("a restore command failure shows the typed error", async () => {
  const apply = vi.fn(async () => {
    throw {
      error: { code: "recovery_writer_active" },
      diagnostic: { code: "wal_lock", message: "the WAL index is locked" },
    };
  });
  renderView(undefined, {
    planRestore: vi.fn(async () => ({ planToken: "fr-op-3" })),
    applyFixtureRecovery: apply,
  });
  await userEvent.click(
    await screen.findByRole("button", { name: "Start Restore" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "the WAL index is locked",
  );
});

test("Safety Snapshots are listed with explicit deletion", async () => {
  const snapshots = [
    {
      snapshotId: "skill-man.snapshot-fr-1",
      path: "/tmp/skill-man.snapshot-fr-1",
      manifestHash: null,
      fileCount: 3,
      totalBytes: 1024,
      takenAt: "2026-08-13T10:00:00Z",
    },
  ];
  const deletePreview = vi.fn(async () => ({
    snapshotId: "skill-man.snapshot-fr-1",
    path: "/tmp/skill-man.snapshot-fr-1",
    fileCount: 3,
    totalBytes: 1024,
  }));
  const deleteApply = vi.fn(async () => undefined);
  renderView(undefined, {
    listSafetySnapshots: vi.fn(async () => snapshots),
    planDeleteSafetySnapshot: deletePreview,
    applyDeleteSafetySnapshot: deleteApply,
  });
  expect(
    await screen.findByText("skill-man.snapshot-fr-1"),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: /Delete/ }));
  await userEvent.click(
    screen.getByRole("button", { name: "Delete permanently" }),
  );
  await waitFor(() =>
    expect(deletePreview).toHaveBeenCalledWith("skill-man.snapshot-fr-1"),
  );
  await waitFor(() =>
    expect(deleteApply).toHaveBeenCalledWith("skill-man.snapshot-fr-1"),
  );
});
