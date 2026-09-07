import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { CatalogClient } from "../../app/catalog-client";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocaleProvider } from "../locale/LocaleProvider";
import { LifecycleRoute } from "./LifecycleRoute";

const BOUND = {
  state: "bound" as const,
  homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
  catalogAccess: "read_write" as const,
  catalogReadonlyReason: null,
  snapshotVersion: 1,
};

const UNAVAILABLE = {
  state: "home_unavailable" as const,
  homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
  path: "/Volumes/Offline/skill-man",
  diagnostic: { code: "volume_unreachable", message: "volume offline" },
};

const MISMATCH = {
  state: "home_identity_mismatch" as const,
  homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
  path: "/Volumes/Other/skill-man",
  diagnostic: { code: "volume_identity_mismatch", message: "different volume" },
};

function renderRoute(
  snapshot: typeof UNAVAILABLE | typeof MISMATCH,
  overrides: Partial<CatalogClient> = {},
) {
  const client = createFixtureCatalogClient();
  Object.assign(client, overrides);
  const onSnapshot = vi.fn();
  const onRetry = vi.fn();
  render(
    <LocaleProvider client={client}>
      <LifecycleRoute
        client={client}
        snapshot={snapshot}
        onSnapshot={onSnapshot}
        onRetry={onRetry}
      />
    </LocaleProvider>,
  );
  return { client, onSnapshot, onRetry };
}

test("home_unavailable exposes Retry, Reconnect, Restore probe and Abandon", async () => {
  renderRoute(UNAVAILABLE);
  const route = await screen.findByRole("status");
  expect(route).toHaveTextContent("Home Unavailable");
  expect(route).toHaveTextContent("volume_unreachable");
  expect(
    screen.getByRole("button", { name: "Reconnect Same Home" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Restore Bound Home" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Abandon Home and Start New" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("navigation", { name: "Library" }),
  ).not.toBeInTheDocument();
});

test("home_identity_mismatch never offers Restore, only Reconnect and Abandon", async () => {
  renderRoute(MISMATCH);
  const route = await screen.findByRole("status");
  expect(route).toHaveTextContent("Home Identity Mismatch");
  expect(
    screen.getByRole("button", { name: "Reconnect Same Home" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Restore Bound Home" }),
  ).not.toBeInTheDocument();
});

test("Reconnect success publishes the Bound snapshot", async () => {
  const reconnect = vi.fn(async () => BOUND);
  const { onSnapshot } = renderRoute(UNAVAILABLE, {
    reconnectSameHome: reconnect,
  });
  await userEvent.click(
    await screen.findByRole("button", { name: "Reconnect Same Home" }),
  );
  await waitFor(() => expect(reconnect).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(onSnapshot).toHaveBeenCalledWith(BOUND));
});

test("Reconnect failure keeps the closed state and reports it", async () => {
  const reconnect = vi.fn(async () => UNAVAILABLE);
  const { onSnapshot } = renderRoute(UNAVAILABLE, {
    reconnectSameHome: reconnect,
  });
  await userEvent.click(
    await screen.findByRole("button", { name: "Reconnect Same Home" }),
  );
  await waitFor(() => expect(onSnapshot).toHaveBeenCalledWith(UNAVAILABLE));
  expect(await screen.findByText(/could not restore/)).toBeInTheDocument();
});

test("Reconnect command failure surfaces the typed error", async () => {
  const reconnect = vi.fn(async () => {
    throw { error: { code: "reconnect_not_available" }, diagnostic: null };
  });
  renderRoute(UNAVAILABLE, { reconnectSameHome: reconnect });
  await userEvent.click(
    await screen.findByRole("button", { name: "Reconnect Same Home" }),
  );
  expect(await screen.findByRole("alert")).toBeInTheDocument();
});

test("Restore probe with a not-applicable reason shows the closed notice", async () => {
  const probe = vi.fn(async () => ({
    kind: "not_applicable" as const,
    reason: "identity_mismatch" as const,
  }));
  renderRoute(UNAVAILABLE, { getRestoreEligibility: probe });
  await userEvent.click(
    await screen.findByRole("button", { name: "Restore Bound Home" }),
  );
  expect(
    await screen.findByText(/does not match the original Home/),
  ).toBeInTheDocument();
});

test("Restore probe with an eligible result opens the Restore route", async () => {
  const probe = vi.fn(async () => ({
    kind: "restore_required" as const,
    homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
    path: "/Volumes/Offline/skill-man",
    reason: "catalog_integrity_failed" as const,
  }));
  renderRoute(UNAVAILABLE, {
    getRestoreEligibility: probe,
    listSafetySnapshots: vi.fn(async () => []),
  });
  await userEvent.click(
    await screen.findByRole("button", { name: "Restore Bound Home" }),
  );
  expect(
    await screen.findByRole("heading", { name: "Restore Bound Home" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Start Restore" }),
  ).toBeInTheDocument();
});

test("Abandon opens the high-friction dialog", async () => {
  const plan = vi.fn(async () => ({
    homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
    path: "/Volumes/Offline/skill-man",
    boundAt: "2026-08-01T00:00:00Z",
    planToken: "ab-1",
  }));
  renderRoute(UNAVAILABLE, { planAbandon: plan });
  await userEvent.click(
    await screen.findByRole("button", { name: "Abandon Home and Start New" }),
  );
  expect(await screen.findByRole("dialog")).toBeInTheDocument();
  expect(plan).toHaveBeenCalledTimes(1);
  expect(
    screen.getByLabelText(/Type the Home ID to continue/),
  ).toBeInTheDocument();
});

test("Retry re-queries the snapshot", async () => {
  const { onRetry } = renderRoute(UNAVAILABLE);
  await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
  expect(onRetry).toHaveBeenCalledTimes(1);
});
