import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { BootstrapApp } from "./BootstrapApp";
import {
  createCatalogClient,
  type BootstrapChangedPayload,
  type BootstrapSnapshot,
  type CatalogClient,
} from "./catalog-client";

function clientWithSnapshot(snapshot: BootstrapSnapshot): CatalogClient {
  const client = createFixtureCatalogClient();
  client.getBootstrapSnapshot = vi.fn(async () => snapshot);
  return client;
}

test("bound snapshot renders the Library Desk as the single top-level route", async () => {
  const client = createFixtureCatalogClient();
  render(<BootstrapApp client={client} />);

  expect(
    await screen.findByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "Unconfigured" }),
  ).not.toBeInTheDocument();
});

test("unconfigured snapshot renders the Unconfigured route", async () => {
  render(
    <BootstrapApp client={clientWithSnapshot({ state: "unconfigured" })} />,
  );

  const route = await screen.findByRole("status");
  expect(route).toHaveTextContent("Unconfigured");
  expect(
    screen.queryByRole("navigation", { name: "Library" }),
  ).not.toBeInTheDocument();
});

test("app_state_unavailable route shows the raw diagnostic and Retry re-resolves", async () => {
  const client = clientWithSnapshot({
    state: "app_state_unavailable",
    diagnostic: {
      code: "locator_invalid",
      message: "home-binding.json could not be parsed",
    },
  });
  render(<BootstrapApp client={client} />);

  const route = await screen.findByRole("status");
  expect(route).toHaveTextContent("App State Unavailable");
  expect(route).toHaveTextContent("locator_invalid");

  // Retry re-queries the native authority.
  const getSnapshot = vi.mocked(client.getBootstrapSnapshot);
  expect(getSnapshot).toHaveBeenCalledTimes(1);
  client.getBootstrapSnapshot = vi.fn<CatalogClient["getBootstrapSnapshot"]>(
    async () => ({ state: "unconfigured" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("status")).toHaveTextContent("Unconfigured");
});

test("home_identity_mismatch route never treats the site as Bound", async () => {
  render(
    <BootstrapApp
      client={clientWithSnapshot({
        state: "home_identity_mismatch",
        homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
        path: "/Volumes/Other/skill-man",
        diagnostic: {
          code: "catalog_identity_mismatch",
          message: "the Catalog identity does not match the binding",
        },
      })}
    />,
  );

  const route = await screen.findByRole("status");
  expect(route).toHaveTextContent("Home Identity Mismatch");
  expect(route).toHaveTextContent("catalog_identity_mismatch");
  expect(
    screen.queryByRole("navigation", { name: "Library" }),
  ).not.toBeInTheDocument();
});

test("bootstrap://changed events re-render the top-level route", async () => {
  const listener: {
    emit: ((payload: BootstrapChangedPayload) => void) | null;
  } = { emit: null };
  const client = createFixtureCatalogClient();
  client.getBootstrapSnapshot = vi.fn<CatalogClient["getBootstrapSnapshot"]>(
    async () => ({ state: "unconfigured" }),
  );
  client.listenBootstrapChanged = vi.fn(
    async (callback: (payload: BootstrapChangedPayload) => void) => {
      listener.emit = callback;
      return () => {};
    },
  );
  render(<BootstrapApp client={client} />);
  expect(await screen.findByRole("status")).toHaveTextContent("Unconfigured");

  // A native event flips the route to Bound without a re-query.
  listener.emit?.({
    snapshot: {
      state: "bound",
      homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
      catalogAccess: "read_write",
      catalogReadonlyReason: null,
      snapshotVersion: 1,
    },
    generation: 2,
  });
  expect(
    await screen.findByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
  expect(client.getBootstrapSnapshot).toHaveBeenCalledTimes(1);
});

test("the production factory fails closed outside the Tauri runtime", async () => {
  const client = createCatalogClient();
  const snapshot = await client.getBootstrapSnapshot();
  expect(snapshot).toEqual({
    state: "app_state_unavailable",
    diagnostic: {
      code: "no_tauri_runtime",
      message: "Skill Man is not running in the Tauri runtime.",
    },
  });
  await expect(client.listSkills("all")).rejects.toMatchObject({
    code: "bootstrap_unavailable",
  });
});
