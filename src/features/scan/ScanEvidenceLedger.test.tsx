import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { ScanEvidenceLedger } from "./ScanEvidenceLedger";

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
