import { BackgroundOperations } from "../../ui/BackgroundOperations";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { ScanEvidenceLedger } from "./ScanEvidenceLedger";

import {
  COMPLETE_SUMMARY,
  LOCAL_CANDIDATE_ROW,
} from "../../test-fixtures/scan-report";

test("never-scanned surface shows the honest state and offers Rescan", async () => {
  render(
    <BackgroundOperations>
      <ScanEvidenceLedger client={createFixtureCatalogClient()} />
    </BackgroundOperations>,
  );
  expect(await screen.findByText("Never scanned")).toBeInTheDocument();
  expect(screen.queryByText(/Stale report/)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Rescan" })).toBeInTheDocument();
});

test("Rescan action shows background progress with real phase facts", async () => {
  const user = userEvent.setup();
  render(
    <BackgroundOperations>
      <ScanEvidenceLedger client={createFixtureCatalogClient()} />
    </BackgroundOperations>,
  );
  await screen.findByText("Never scanned");
  await user.click(screen.getByRole("button", { name: "Rescan" }));
  await waitFor(() => {
    expect(screen.getAllByText("Scanning").length).toBeGreaterThan(0);
  });
  expect(await screen.findByText(/Walking roots/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  // Progress carries real facts, never a percent or an ETA.
  expect(screen.queryByText(/percent/i)).not.toBeInTheDocument();
  expect(screen.queryByText(/eta/i)).not.toBeInTheDocument();
});

test("cancel coordinates with the running Run", async () => {
  const user = userEvent.setup();
  render(
    <BackgroundOperations>
      <ScanEvidenceLedger client={createFixtureCatalogClient()} />
    </BackgroundOperations>,
  );
  await screen.findByText("Never scanned");
  await user.click(screen.getByRole("button", { name: "Rescan" }));
  await screen.findAllByText("Scanning");
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

test("Rescan explains a closed write gate instead of reporting an internal error", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.startRescan = async () => {
    throw { error: { code: "scan_not_writable" } };
  };
  render(<ScanEvidenceLedger client={client} />);
  await user.click(await screen.findByRole("button", { name: "Rescan" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Library changes are disabled. Check the Home status first.",
  );
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
