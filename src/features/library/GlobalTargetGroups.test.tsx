import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { GlobalTargetGroupsPanel } from "./GlobalTargetGroups";

function renderPanel({
  skillId = "skill-authoring",
  client = createFixtureCatalogClient(),
}: {
  skillId?: string;
  client?: ReturnType<typeof createFixtureCatalogClient>;
} = {}) {
  return render(
    <GlobalTargetGroupsPanel
      ref={(element) => void element}
      dialog={false}
      skillId={skillId}
      client={client}
      onOpenAgentManagement={() => {}}
    />,
  );
}

test("group cards merge consumers and show one desired state per Target", async () => {
  renderPanel();
  const panel = await screen.findByRole("complementary", {
    name: "Activation Target Groups",
  });
  // The fixture Skill is enabled for Claude Code; Codex shows disabled.
  await waitFor(() => {
    expect(within(panel).getByText("Claude Code")).toBeInTheDocument();
  });
  expect(
    within(panel).getByRole("switch", { name: "Claude Code" }),
  ).toBeChecked();
  expect(
    within(panel).getByRole("switch", { name: "Workbench" }),
  ).not.toBeChecked();
  expect(
    within(panel).getByText(/Project-level links are independent/),
  ).toBeInTheDocument();
});

test("toggling an off group enables it and the result window offers Undo", async () => {
  const user = userEvent.setup();
  renderPanel();
  const panel = await screen.findByRole("complementary", {
    name: "Activation Target Groups",
  });
  const workbenchSwitch = await within(panel).findByRole("switch", {
    name: "Workbench",
  });
  expect(workbenchSwitch).not.toBeChecked();
  await user.click(workbenchSwitch);
  await waitFor(() => {
    expect(
      within(panel).getByRole("button", { name: "Undo this operation" }),
    ).toBeInTheDocument();
  });
  expect(
    within(panel).getByRole("switch", { name: "Workbench" }),
  ).toBeChecked();
});
