import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { LocalMigrationDialog } from "./LocalMigrationDialog";

test("migration errors stay in the dialog and confirmation is explicit", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const plan = vi
    .spyOn(client, "planAdopt")
    .mockRejectedValueOnce({ error: { code: "plan_stale" } })
    .mockResolvedValue({
      planToken: "fresh",
      reportGeneration: 1,
      canApply: true,
      items: [],
    });
  const apply = vi.spyOn(client, "applyAdopt").mockResolvedValue({
    operationId: "operation",
    snapshotVersion: 2,
    undoAvailable: true,
    items: [
      { skillId: "skill", directoryName: "demo", adopted: true, error: null },
    ],
  });
  const rescan = vi.spyOn(client, "startRescan");
  const resolved = vi.fn();
  render(
    <LocalMigrationDialog
      client={client}
      entityRef="entity"
      generation={1}
      name="demo"
      pickDirectory={async () => "/external/skills"}
      formatError={String}
      onResolved={resolved}
      onClose={vi.fn()}
    />,
  );
  const dialog = within(screen.getByRole("dialog"));
  await user.click(dialog.getByRole("button", { name: /Choose folder/ }));
  expect(plan).not.toHaveBeenCalled();
  await user.click(dialog.getByRole("button", { name: "Plan Adopt" }));
  expect(await dialog.findByRole("alert")).toHaveTextContent(
    "Close this dialog and rescan",
  );
  expect(apply).not.toHaveBeenCalled();
  await user.click(dialog.getByRole("button", { name: "Plan Adopt" }));
  expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
  await user.click(dialog.getByRole("button", { name: "Confirm migration" }));
  expect(apply).toHaveBeenCalledWith("fresh");
  expect(resolved).toHaveBeenCalledWith(true);
  expect(rescan).not.toHaveBeenCalled();
});
