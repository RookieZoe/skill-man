import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { CatalogClient } from "../../app/catalog-client";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocaleProvider } from "../locale/LocaleProvider";
import { AbandonFlow } from "./AbandonFlow";

const PREVIEW = {
  homeId: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
  path: "/Volumes/Offline/skill-man",
  boundAt: "2026-08-01T00:00:00Z",
  planToken: "ab-1",
};

function renderFlow(
  overrides: Partial<CatalogClient> = {},
  plan: CatalogClient["planAbandon"] = async () => PREVIEW,
) {
  const client = createFixtureCatalogClient();
  Object.assign(client, { planAbandon: plan, ...overrides });
  const onSnapshot = vi.fn();
  const onClose = vi.fn();
  render(
    <LocaleProvider client={client}>
      <AbandonFlow client={client} onSnapshot={onSnapshot} onClose={onClose} />
    </LocaleProvider>,
  );
  return { client, onSnapshot, onClose };
}

test("the typed Home ID must match exactly before Continue enables", async () => {
  renderFlow();
  const input = await screen.findByLabelText(/Type the Home ID to continue/);
  expect(screen.getByText(/The ID must match exactly/)).toBeInTheDocument();

  const continueButton = screen.getByRole("button", { name: "Continue" });
  expect(continueButton).toBeDisabled();

  await userEvent.type(input, "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ac");
  expect(continueButton).toBeDisabled();

  await userEvent.clear(input);
  await userEvent.type(input, PREVIEW.homeId);
  expect(continueButton).toBeEnabled();
});

test("the second explicit confirmation applies the Abandon and publishes the snapshot", async () => {
  const apply = vi.fn(async () => ({
    state: "unconfigured" as const,
  }));
  const { onSnapshot, onClose } = renderFlow({ applyAbandon: apply });

  const input = await screen.findByLabelText(/Type the Home ID to continue/);
  await userEvent.type(input, PREVIEW.homeId);
  await userEvent.click(screen.getByRole("button", { name: "Continue" }));

  // Second confirmation step: explicit Abandon button, apply still not run.
  const abandonButton = screen.getByRole("button", {
    name: "Abandon Home and Start New",
  });
  expect(abandonButton).toBeEnabled();
  expect(apply).not.toHaveBeenCalled();

  await userEvent.click(abandonButton);
  await waitFor(() =>
    expect(apply).toHaveBeenCalledWith("ab-1", PREVIEW.homeId),
  );
  await waitFor(() =>
    expect(onSnapshot).toHaveBeenCalledWith(
      expect.objectContaining({ state: "unconfigured" }),
    ),
  );
  await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
});

test("a CAS race failure keeps the dialog open with the typed error", async () => {
  const apply = vi.fn(async () => {
    throw {
      error: { code: "abandon_cas_conflict" },
      diagnostic: { code: "abandon_cas_conflict", message: "locator changed" },
    };
  });
  const { onSnapshot, onClose } = renderFlow({ applyAbandon: apply });

  const input = await screen.findByLabelText(/Type the Home ID to continue/);
  await userEvent.type(input, PREVIEW.homeId);
  await userEvent.click(screen.getByRole("button", { name: "Continue" }));
  await userEvent.click(
    screen.getByRole("button", { name: "Abandon Home and Start New" }),
  );
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent(/Home settings changed/);
  expect(onSnapshot).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();

  // Start over re-plans the Abandon.
  await userEvent.click(screen.getByRole("button", { name: "Start over" }));
  await waitFor(() =>
    expect(
      screen.getByLabelText(/Type the Home ID to continue/),
    ).toBeInTheDocument(),
  );
});

test("plan failure surfaces the error and allows closing", async () => {
  const plan = vi.fn(async () => {
    throw {
      error: { code: "abandon_not_applicable" },
      diagnostic: { code: "abandon_not_applicable", message: "no binding" },
    };
  });
  const { onClose } = renderFlow({}, plan);
  expect(await screen.findByText(/cannot be abandoned/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("Escape closes the dialog unless busy", async () => {
  const { onClose } = renderFlow();
  await screen.findByLabelText(/Type the Home ID to continue/);
  await userEvent.keyboard("{Escape}");
  expect(onClose).toHaveBeenCalledTimes(1);
});
