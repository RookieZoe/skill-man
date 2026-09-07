import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import type {
  ObservationAndScanSnapshot,
  ScanRunState,
} from "../app/catalog-client";
import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { COMPLETE_SUMMARY } from "../test-fixtures/scan-report";
import { BackgroundOperations } from "./BackgroundOperations";
import { useScanOperation } from "./useScanOperation";

function Observer({ snapshot }: { snapshot: ObservationAndScanSnapshot }) {
  useScanOperation(snapshot);
  return null;
}

test("only observed runs complete, incomplete and cancelled scans never report success", async () => {
  const base = await createFixtureCatalogClient().getObservationSnapshot();
  function snapshot(
    state: ScanRunState,
    incomplete = false,
  ): ObservationAndScanSnapshot {
    return {
      ...base,
      scanRun: {
        runId: COMPLETE_SUMMARY.runId,
        generation: 5,
        trigger: "manual",
        state,
        phase: "walking",
        currentRoot: null,
        counts: COMPLETE_SUMMARY.counts,
        roots: [],
        elapsedMs: 0,
        slow: false,
        diagnostic: null,
      },
      currentReport: {
        ...base.currentReport,
        summary: {
          ...COMPLETE_SUMMARY,
          incomplete,
          state: incomplete ? "incomplete" : "complete",
        },
      },
    };
  }
  const view = (state: ScanRunState, incomplete = false) => (
    <BackgroundOperations>
      <Observer snapshot={snapshot(state, incomplete)} />
    </BackgroundOperations>
  );
  const { rerender } = render(view("completed"));
  expect(screen.queryByText("Scan completed")).not.toBeInTheDocument();
  rerender(view("running"));
  await screen.findByRole("progressbar", { name: "Scanning" });
  rerender(view("completed", true));
  expect(screen.getByText("Scan incomplete")).toBeInTheDocument();
  expect(screen.queryByText("Scan completed")).not.toBeInTheDocument();
  rerender(view("running"));
  rerender(view("cancelled"));
  expect(screen.getByText("Scan cancelled")).toBeInTheDocument();
  rerender(view("running"));
  rerender(view("completed"));
  expect(screen.getByText("Scan completed")).toBeInTheDocument();
  expect(
    screen.getByText(/Open the scan report to review/),
  ).toBeInTheDocument();
});
