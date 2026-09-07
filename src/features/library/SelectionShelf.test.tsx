import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { SelectionShelf } from "./SelectionShelf";
import { App } from "../../app/App";

test("SelectionShelf renders nothing when selectedCount is 0", () => {
  const { container } = render(
    <SelectionShelf
      selectedCount={0}
      onEnableGlobally={() => {}}
      onEnableToProject={() => {}}
      onExit={() => {}}
    />,
  );
  expect(container.firstChild).toBeNull();
});

test("batch disable becomes available when any selected Skill is enabled", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();
  await user.click(screen.getByRole("button", { name: "Batch actions" }));
  await user.click(screen.getByRole("button", { name: "Select all" }));
  const button = screen.getByRole("button", {
    name: "Disable globally…",
  });
  expect(button).toBeEnabled();
  await user.click(button);
  expect(
    await screen.findByRole("dialog", { name: "Disable globally…" }),
  ).toBeVisible();
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Close" })).toBeEnabled(),
  );
  await user.click(screen.getByRole("button", { name: "Close" }));
  await user.click(screen.getByRole("button", { name: "Clear selection" }));
  expect(
    screen.queryByRole("region", { name: /skills selected/ }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("checkbox", { name: "legacy-audit" }));
  expect(
    screen.getByRole("button", { name: "Disable globally…" }),
  ).toBeDisabled();
});

test("SelectionShelf renders count and triggers action callbacks when selectedCount >= 1", async () => {
  const user = userEvent.setup();
  const onEnableGlobally = vi.fn();
  const onEnableToProject = vi.fn();
  const onExit = vi.fn();

  render(
    <SelectionShelf
      selectedCount={2}
      onEnableGlobally={onEnableGlobally}
      onEnableToProject={onEnableToProject}
      onExit={onExit}
    />,
  );

  const shelf = screen.getByRole("region", { name: "2 skills selected" });
  expect(shelf).toBeInTheDocument();
  expect(within(shelf).getByText("2 skills selected")).toBeInTheDocument();

  await user.click(
    within(shelf).getByRole("button", { name: "Enable Globally…" }),
  );
  expect(onEnableGlobally).toHaveBeenCalledTimes(1);

  await user.click(
    within(shelf).getByRole("button", { name: "Enable to Project…" }),
  );
  expect(onEnableToProject).toHaveBeenCalledTimes(1);

  await user.click(within(shelf).getByRole("button", { name: "Exit" }));
  expect(onExit).toHaveBeenCalledTimes(1);
});

test("LibraryDesk multi-select: Select button enters mode, toggles skills, shelf provides batch enable and exit", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<App client={client} />);

  // Wait for library to load
  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();

  // 1. Enter multi-select mode via Toolbar Select button
  const selectBtn = screen.getByRole("button", { name: "Batch actions" });
  expect(selectBtn).toHaveAttribute("aria-pressed", "false");
  await user.click(selectBtn);
  expect(selectBtn).toHaveAttribute("aria-pressed", "true");

  // Initial selection is empty; shelf is not yet visible
  expect(
    screen.queryByRole("region", { name: /skills? selected/ }),
  ).not.toBeInTheDocument();

  // Checkboxes are now present on skill rows
  const checkboxes = screen.getAllByRole("checkbox");
  expect(checkboxes.length).toBeGreaterThan(1);
  expect(checkboxes[0]).not.toBeChecked();

  // 2. Select first skill
  await user.click(checkboxes[0]);
  expect(checkboxes[0]).toBeChecked();

  // Shelf appears with 1 skill selected
  const shelf1 = await screen.findByRole("region", {
    name: "1 skill selected",
  });
  expect(shelf1).toBeInTheDocument();

  // Select second skill
  await user.click(checkboxes[1]);
  expect(checkboxes[1]).toBeChecked();
  expect(
    screen.getByRole("region", { name: "2 skills selected" }),
  ).toBeInTheDocument();

  // 3. Exit clears draft and exits multi-select mode
  const exitBtn = screen.getByRole("button", { name: "Exit" });
  await user.click(exitBtn);

  // Shelf disappears and Select button is no longer pressed
  expect(
    screen.queryByRole("region", { name: /skills? selected/ }),
  ).not.toBeInTheDocument();
  expect(selectBtn).toHaveAttribute("aria-pressed", "false");
  expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
});

test("LibraryDesk multi-select: opens Global Enable sheet in batch mode and clears on close", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();

  // Enter select mode and select 2 skills
  await user.click(screen.getByRole("button", { name: "Batch actions" }));
  const checkboxes = screen.getAllByRole("checkbox");
  await user.click(checkboxes[0]);
  await user.click(checkboxes[1]);

  const shelf = screen.getByRole("region", { name: "2 skills selected" });
  await user.click(
    within(shelf).getByRole("button", { name: "Enable Globally…" }),
  );

  // Global Enable sheet opens with batch dialog label
  const dialog = await screen.findByRole("dialog", {
    name: "Enable 2 skills globally",
  });
  expect(dialog).toBeInTheDocument();

  // Close sheet with Escape
  await user.keyboard("{Escape}");
  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  // Multi-select draft is cleared on close (does not remember across operations)
  expect(
    screen.queryByRole("region", { name: /skills? selected/ }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Batch actions" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

test("LibraryDesk multi-select: opens Project Enable sheet in batch mode and clears on close", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();

  // Enter select mode and select 2 skills
  await user.click(screen.getByRole("button", { name: "Batch actions" }));
  const checkboxes = screen.getAllByRole("checkbox");
  await user.click(checkboxes[0]);
  await user.click(checkboxes[1]);

  const shelf = screen.getByRole("region", { name: "2 skills selected" });
  await user.click(
    within(shelf).getByRole("button", { name: "Enable to Project…" }),
  );

  // Project Enable sheet opens with batch dialog label
  const dialog = await screen.findByRole("dialog", {
    name: "Enable 2 skills to Project",
  });
  expect(dialog).toBeInTheDocument();

  // Close sheet with Escape
  await user.keyboard("{Escape}");
  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  // Multi-select draft is cleared on close
  expect(
    screen.queryByRole("region", { name: /skills? selected/ }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Batch actions" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});
import { expandLibrary } from "../../test-fixtures/expand-library";
