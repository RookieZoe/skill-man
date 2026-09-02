import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { AgentManagement } from "./AgentManagement";

test("fresh Home renders empty configured state and all zero-write presets", async () => {
  render(
    <AgentManagement
      client={createFixtureCatalogClient({ emptyAgentConfigurations: true })}
      layoutMode="wide"
    />,
  );

  expect(
    await screen.findByRole("heading", { name: "No Agent Configuration yet" }),
  ).toBeInTheDocument();
  const navigation = screen.getByRole("navigation", { name: "Agent groups" });
  expect(within(navigation).getByText("Configured")).toBeInTheDocument();
  expect(
    within(navigation).getByText("Detected but unconfigured"),
  ).toBeInTheDocument();
  expect(within(navigation).getByText("Preset templates")).toBeInTheDocument();
  expect(
    screen.getByText("Detection never writes the Catalog or creates folders."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("switch")).not.toBeInTheDocument();

  await userEvent.click(within(navigation).getByText("Preset templates"));
  expect(
    await screen.findByRole("button", { name: /Claude Code/ }),
  ).toBeInTheDocument();
});

test("mid drawer traps Escape and focuses its close control", async () => {
  const user = userEvent.setup();
  render(
    <AgentManagement client={createFixtureCatalogClient()} layoutMode="mid" />,
  );
  const row = await screen.findByRole("button", { name: /Claude Code/ });
  await user.click(row);

  const drawer = screen.getByRole("dialog", {
    name: "Agent configuration details",
  });
  const close = within(drawer).getByRole("button", { name: "Close details" });
  expect(close).toHaveFocus();

  await user.keyboard("{Escape}");
  expect(
    screen.queryByRole("dialog", { name: "Agent configuration details" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Open details" }),
  ).toBeInTheDocument();
});

test("single configuration sheet edits multiple roots with one radio target", async () => {
  const user = userEvent.setup();
  render(
    <AgentManagement
      client={createFixtureCatalogClient({ emptyAgentConfigurations: true })}
      layoutMode="wide"
    />,
  );
  await screen.findByRole("heading", { name: "No Agent Configuration yet" });
  await user.click(
    screen.getAllByRole("button", { name: "New custom agent" })[0],
  );

  const dialog = screen.getByRole("dialog", { name: "Agent Configuration" });
  await user.type(within(dialog).getByLabelText("Agent name"), "Writer");
  await user.type(
    within(dialog).getByLabelText("Global Skills Root 1"),
    "/Users/zoe/.writer/scan",
  );
  await user.click(
    within(dialog).getByRole("button", { name: "Add Global Skills Root" }),
  );
  await user.type(
    within(dialog).getByLabelText("Global Skills Root 2"),
    "/Users/zoe/.writer/target",
  );
  const radios = within(dialog).getAllByRole("radio");
  expect(radios[0]).toBeChecked();
  await user.click(radios[1]);
  expect(radios[0]).not.toBeChecked();
  expect(radios[1]).toBeChecked();
  expect(
    radios.filter((radio) => (radio as HTMLInputElement).checked),
  ).toHaveLength(1);
  await user.type(
    within(dialog).getByLabelText("Project skills directory, optional"),
    ".writer/skills",
  );

  await user.click(within(dialog).getByRole("button", { name: "Review" }));
  expect(await within(dialog).findByText("Core review")).toBeInTheDocument();
  await user.click(
    within(dialog).getByRole("button", { name: "Create configuration" }),
  );

  await waitFor(() => {
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  expect(screen.getByRole("button", { name: /Writer/ })).toBeInTheDocument();
});

test("mid layout opens configuration details in the shared drawer contract", async () => {
  const user = userEvent.setup();
  render(
    <AgentManagement client={createFixtureCatalogClient()} layoutMode="mid" />,
  );
  const row = await screen.findByRole("button", { name: /Claude Code/ });
  await user.click(row);

  const drawer = screen.getByRole("dialog", {
    name: "Agent configuration details",
  });
  expect(drawer).toHaveTextContent("Compatibility evidence");
  expect(drawer).toHaveTextContent("Agent Activation Target");
  await user.click(
    within(drawer).getByRole("button", { name: "Close details" }),
  );
  expect(
    screen.getByRole("button", { name: "Open details" }),
  ).toBeInTheDocument();
});
