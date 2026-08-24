import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type {
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
  ExistingHomeRecoveryPlan,
} from "../../app/catalog-client";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocaleProvider } from "../locale/LocaleProvider";
import { HomeBindingView } from "./HomeBindingView";

function renderView(
  snapshot:
    | Extract<BootstrapSnapshot, { state: "unconfigured" }>
    | Extract<BootstrapSnapshot, { state: "legacy_detected" }>
    | Extract<BootstrapSnapshot, { state: "home_candidate_pending" }>,
  overrides: Partial<CatalogClient> = {},
  pickDirectory?: () => Promise<string | null>,
) {
  const client = createFixtureCatalogClient();
  Object.assign(client, overrides);
  const onSnapshot = vi.fn();
  render(
    <LocaleProvider client={client}>
      <HomeBindingView
        client={client}
        snapshot={snapshot}
        onSnapshot={onSnapshot}
        pickDirectory={pickDirectory}
      />
    </LocaleProvider>,
  );
  return { client, onSnapshot };
}

test("unconfigured offers Use Default and Choose…", async () => {
  renderView({ state: "unconfigured" });
  expect(
    await screen.findByRole("button", {
      name: /Use Default/,
    }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Choose/ })).toBeInTheDocument();
});

test("Use Default prepares the default path then requires explicit confirmation", async () => {
  const prepare = vi.fn(async (path: string) => {
    expect(path).toBe("");
    return {
      path: "~/Library/Application Support/skill-man",
      token: "hb-1",
      mode: "fresh" as const,
      volumeFsid: "fsid",
      volumeUuid: "uuid",
      availableBytes: 1024 * 1024 * 1024,
      legacySource: null,
    };
  });
  const confirm = vi.fn(async () => ({
    state: "bound" as const,
    homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
    catalogAccess: "read_write" as const,
    catalogReadonlyReason: null,
    snapshotVersion: 1,
  }));
  const { onSnapshot } = renderView(
    { state: "unconfigured" },
    { prepareHome: prepare, confirmHome: confirm },
  );

  await userEvent.click(
    await screen.findByRole("button", { name: /Use Default/ }),
  );
  // The candidate needs an explicit confirmation before any binding.
  expect(
    await screen.findByRole("heading", { name: "Confirm your Home" }),
  ).toBeInTheDocument();
  expect(prepare).toHaveBeenCalledTimes(1);
  expect(confirm).not.toHaveBeenCalled();

  await userEvent.click(screen.getByRole("button", { name: "Bind Home" }));
  await waitFor(() => expect(confirm).toHaveBeenCalledWith("hb-1"));
  await waitFor(() =>
    expect(onSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({ state: "bound" }),
    ),
  );
});

test("Choose… uses the injected directory picker", async () => {
  const prepare = vi.fn(async () => {
    throw {
      error: { code: "candidate_invalid", reason: "not_empty" },
    } as CommandFailure;
  });
  const pickDirectory = vi.fn(async () => "/Users/test/My Home");
  renderView(
    { state: "unconfigured" },
    { prepareHome: prepare },
    pickDirectory,
  );

  await userEvent.click(await screen.findByRole("button", { name: /Choose/ }));
  await waitFor(() => expect(pickDirectory).toHaveBeenCalledTimes(1));
  await waitFor(() =>
    expect(prepare).toHaveBeenCalledWith("/Users/test/My Home"),
  );
});

test("Recover Existing Home confirms the reviewed plan without asking for a Home ID", async () => {
  const prepareRecovery = vi.fn(
    async (path: string): Promise<ExistingHomeRecoveryPlan> => {
      expect(path).toBe("/Users/test/Recovered Home");
      return {
        path,
        homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
        createdAt: "2026-08-01T00:00:00Z",
        planToken: "ehr-1",
        facts: ["marker_catalog_identity", "catalog_integrity"],
      };
    },
  );
  const cancelRecovery = vi.fn(async () => undefined);
  const confirmRecovery = vi.fn(async () => ({
    state: "bound" as const,
    homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
    catalogAccess: "read_write" as const,
    catalogReadonlyReason: null,
    snapshotVersion: 1,
  }));
  const pickDirectory = vi.fn(async () => "/Users/test/Recovered Home");
  const { onSnapshot } = renderView(
    { state: "unconfigured" },
    {
      prepareExistingHomeRecovery: prepareRecovery,
      cancelExistingHomeRecovery: cancelRecovery,
      confirmExistingHomeRecovery: confirmRecovery,
    },
    pickDirectory,
  );

  await userEvent.click(
    await screen.findByRole("button", { name: "Recover Existing Home…" }),
  );

  expect(
    await screen.findByRole("heading", { name: "Recover Existing Home" }),
  ).toBeInTheDocument();
  expect(screen.getByText("/Users/test/Recovered Home")).toBeInTheDocument();
  expect(
    screen.getByText("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab"),
  ).toBeInTheDocument();
  expect(
    screen.getByText("Home marker and Catalog identity agree."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  expect(
    screen.getByText(
      "If previous app history was lost, this can reactivate a Home that was previously abandoned.",
    ),
  ).toBeInTheDocument();

  await userEvent.click(
    screen.getByRole("button", { name: "Recover this Home" }),
  );
  await waitFor(() => expect(confirmRecovery).toHaveBeenCalledWith("ehr-1"));
  await waitFor(() =>
    expect(onSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({ state: "bound" }),
    ),
  );

  expect(cancelRecovery).not.toHaveBeenCalled();
  expect(prepareRecovery).toHaveBeenCalledTimes(1);
});

test("candidate rejection renders a reason-specific message with diagnostics", async () => {
  const prepare = vi.fn(async () => {
    throw {
      error: { code: "candidate_invalid", reason: "not_empty" },
      diagnostic: {
        code: "candidate_invalid",
        message: "/tmp/x: candidate path: /tmp/x",
      },
    } as CommandFailure;
  });
  renderView({ state: "unconfigured" }, { prepareHome: prepare });

  await userEvent.click(
    await screen.findByRole("button", { name: /Use Default/ }),
  );
  expect(
    await screen.findByRole("heading", {
      name: "Home setup could not complete",
    }),
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      "This folder is not empty. Choose an empty folder to set up a new Home.",
    ),
  ).toBeInTheDocument();
  // Raw technical detail stays in the explicitly labeled region.
  expect(
    screen.getByText("/tmp/x: candidate path: /tmp/x"),
  ).toBeInTheDocument();
});

test("legacy detected explains the one-time transition and binds in place", async () => {
  const prepare = vi.fn(async (path: string) => {
    expect(path).toBe("/var/old/skill-man");
    return {
      path: "/var/old/skill-man",
      token: "hb-legacy",
      mode: "legacy_in_place" as const,
      volumeFsid: "fsid",
      volumeUuid: "uuid",
      availableBytes: 1024 * 1024 * 1024,
      legacySource: null,
    };
  });
  renderView(
    { state: "legacy_detected", path: "/var/old/skill-man" },
    { prepareHome: prepare },
  );

  expect(
    await screen.findByText(/A Legacy Home was found/),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: /Use Default/ }));
  expect(await screen.findByText(/bound in place/)).toBeInTheDocument();
});

test("pending candidate routes to Continue and Cancel", async () => {
  const resume = vi.fn(async () => ({
    state: "bound" as const,
    homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
    catalogAccess: "read_write" as const,
    catalogReadonlyReason: null,
    snapshotVersion: 1,
  }));
  const cancel = vi.fn(async () => ({ state: "unconfigured" as const }));
  const { onSnapshot } = renderView(
    {
      state: "home_candidate_pending",
      path: "/tmp/candidate",
      operationId: "hb-crash",
    },
    { continueCandidate: resume, cancelCandidate: cancel },
  );

  expect(
    await screen.findByRole("heading", { name: "Home candidate pending" }),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Continue" }));
  await waitFor(() => expect(resume).toHaveBeenCalledWith("hb-crash"));

  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument(),
  );
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(cancel).toHaveBeenCalledWith("hb-crash"));
  await waitFor(() =>
    expect(onSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({ state: "unconfigured" }),
    ),
  );
});

test("non-cancellable error is shown and back returns to the route", async () => {
  const cancel = vi.fn(async () => {
    throw {
      error: { code: "binding_not_cancellable" },
      diagnostic: {
        code: "binding_not_cancellable",
        message: "the location contains user content",
      },
    } as CommandFailure;
  });
  renderView(
    {
      state: "home_candidate_pending",
      path: "/tmp/candidate",
      operationId: "hb-crash",
    },
    { cancelCandidate: cancel },
  );

  await userEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  expect(
    await screen.findByText(
      "This setup cannot be cancelled: the location contains content that was not created by it.",
    ),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Back" }));
  expect(
    await screen.findByRole("button", { name: "Continue" }),
  ).toBeInTheDocument();
});
