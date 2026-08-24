import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type {
  BootstrapSnapshot,
  DefaultHomeRecoveryBlockedReason,
  ExistingHomeRecoveryPlan,
} from "../../app/catalog-client";
import { BootstrapApp } from "../../app/BootstrapApp";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocaleProvider } from "../locale/LocaleProvider";
import { DefaultHomeRecoveryView } from "./DefaultHomeRecoveryView";

const offer: Extract<
  BootstrapSnapshot,
  { state: "default_home_recovery_offer" }
> = {
  state: "default_home_recovery_offer",
  path: "/Users/test/Library/Application Support/skill-man",
};

test("default recovery offer is reviewed explicitly and cancelling returns to the same offer", async () => {
  const client = createFixtureCatalogClient();
  const prepare = vi.fn(
    async (path: string): Promise<ExistingHomeRecoveryPlan> => ({
      path,
      homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
      createdAt: "2026-08-01T00:00:00Z",
      planToken: "ehr-default",
      facts: ["marker_catalog_identity", "catalog_integrity"],
    }),
  );
  const cancel = vi.fn(async () => undefined);
  client.prepareExistingHomeRecovery = prepare;
  client.cancelExistingHomeRecovery = cancel;
  render(
    <LocaleProvider client={client}>
      <DefaultHomeRecoveryView
        client={client}
        snapshot={offer}
        onSnapshot={vi.fn()}
      />
    </LocaleProvider>,
  );

  expect(
    await screen.findByRole("heading", { name: "Existing Default Home found" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: /Use Default|Choose|Abandon/ }),
  ).not.toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "Review recovery" }),
  );
  await screen.findByRole("heading", { name: "Recover Existing Home" });
  expect(prepare).toHaveBeenCalledWith(offer.path);

  await userEvent.click(screen.getByRole("button", { name: "Back" }));
  await waitFor(() => expect(cancel).toHaveBeenCalledWith("ehr-default"));
  expect(
    await screen.findByRole("heading", { name: "Existing Default Home found" }),
  ).toBeInTheDocument();
});

test("all default recovery blocked diagnostics expose only Retry with no binding bypass", async () => {
  const reasons: DefaultHomeRecoveryBlockedReason[] = [
    "not_directory",
    "marker_missing_or_invalid",
    "layout_capabilities",
    "catalog_missing",
    "catalog_unreadable",
    "catalog_identity_missing",
    "home_identity_mismatch",
    "creation_time_mismatch",
    "catalog_integrity",
    "catalog_foreign_keys",
    "catalog_capabilities",
    "active_writer",
    "operation_recovery_required",
    "fixture_contamination",
    "unreadable",
    "recovery_ineligible",
  ];

  for (const reason of reasons) {
    const client = createFixtureCatalogClient();
    client.getBootstrapSnapshot = vi.fn(async () => ({
      state: "default_home_recovery_blocked" as const,
      path: "/Users/test/Library/Application Support/skill-man",
      reason,
    }));
    const { unmount } = render(<BootstrapApp client={client} />);
    expect(
      await screen.findByRole("heading", {
        name: "Default Home recovery required",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: /Use Default|Choose|Recover Existing Home|Abandon/,
      }),
    ).not.toBeInTheDocument();
    expect(screen.getByText(reason)).toBeInTheDocument();
    unmount();
  }
});
