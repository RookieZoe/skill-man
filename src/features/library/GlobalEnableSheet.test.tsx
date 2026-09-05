import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import type { EnableCell, EnablePlan } from "../../app/catalog-client";
import { GlobalEnableSheet } from "./GlobalEnableSheet";

function renderSheet(initialGroupIds: string[] = []) {
  return render(
    <GlobalEnableSheet
      client={createFixtureCatalogClient()}
      skillId="skill-authoring"
      skillName="Skill authoring"
      initialGroupIds={initialGroupIds}
      onClose={() => {}}
    />,
  );
}

test("three-step flow: select groups, preview matrix, result with undo", async () => {
  const user = userEvent.setup();
  renderSheet();

  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  expect(
    await within(dialog).findByText("No Target group selected", {
      exact: false,
    }),
  ).toBeInTheDocument();
  // The physical target groups appear with their consumer Agents.
  const workbenchGroup = await within(dialog).findByText("Workbench");
  await user.click(workbenchGroup);
  expect(within(dialog).getByText(/selected/)).toBeInTheDocument();

  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );
  const preview = await within(dialog).findByText(/Skill authoring/, {
    selector: ".enable-matrix-cell",
  });
  // Wait for the plan to load (matrix shows the Ready cell).
  expect(await within(dialog).findByText("Ready")).toBeInTheDocument();
  expect(preview).toBeInTheDocument();

  await user.click(
    within(dialog).getByRole("button", { name: "Enable in this Target" }),
  );
  const result = await within(dialog).findByText(/1 of 1 cells succeeded/);
  expect(result).toBeInTheDocument();
  expect(
    within(dialog).getByRole("button", { name: "Undo this operation" }),
  ).toBeInTheDocument();
  expect(within(dialog).getByText(/skill-authoring\|/)).toBeInTheDocument();

  await user.click(
    within(dialog).getByRole("button", { name: "Undo this operation" }),
  );
  await waitFor(() => {
    expect(
      within(dialog).queryByRole("button", { name: "Undo this operation" }),
    ).not.toBeInTheDocument();
  });
});

test("batch flow: multiple skills, batch dialog label, preview cells for all skills, undo", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(
    <GlobalEnableSheet
      client={client}
      skills={[
        { id: "skill-authoring", name: "Skill authoring" },
        { id: "media-xray", name: "Media X-ray" },
      ]}
      onClose={() => {}}
    />,
  );

  const dialog = await screen.findByRole("dialog", {
    name: "Enable 2 skills globally",
  });
  expect(dialog).toBeInTheDocument();

  // Select all target groups
  await user.click(within(dialog).getByRole("button", { name: "Select all" }));
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );

  // Both skills are in the preview matrix
  const authoringMatches =
    await within(dialog).findAllByText(/Skill authoring/);
  expect(authoringMatches.length).toBeGreaterThan(0);
  const mediaMatches = within(dialog).getAllByText(/Media X-ray/);
  expect(mediaMatches.length).toBeGreaterThan(0);

  // Apply
  const applyBtn = within(dialog).getByRole("button", {
    name: "Enable in this Target",
  });
  expect(applyBtn).not.toBeDisabled();
  await user.click(applyBtn);

  // Result
  expect(
    await within(dialog).findByText(/cells succeeded/),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByRole("button", { name: "Undo this operation" }),
  ).toBeInTheDocument();
});

test("batch flow with conflict: only Replace and Skip available", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(
    <GlobalEnableSheet
      client={client}
      skills={[
        { id: "twin-1::twin-tool", name: "Twin Tool 1" },
        { id: "twin-2::twin-tool", name: "Twin Tool 2" },
      ]}
      onClose={() => {}}
    />,
  );

  const dialog = await screen.findByRole("dialog", {
    name: "Enable 2 skills globally",
  });
  await user.click(within(dialog).getByRole("button", { name: "Select all" }));
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );

  const selects = await within(dialog).findAllByRole("combobox");
  expect(selects.length).toBeGreaterThan(0);
  // Verify that "Adopt existing" is not an option in batch mode
  expect(
    within(dialog).queryByRole("option", { name: "Adopt existing" }),
  ).not.toBeInTheDocument();
  expect(
    within(dialog).getAllByRole("option", { name: "Replace" }).length,
  ).toBeGreaterThan(0);
  expect(
    within(dialog).getAllByRole("option", { name: "Skip" }).length,
  ).toBeGreaterThan(0);
});

test("default selection is empty and Select all selects every group", async () => {
  const user = userEvent.setup();
  renderSheet();
  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  expect(
    await within(dialog).findByText("No Target group selected", {
      exact: false,
    }),
  ).toBeInTheDocument();
  await user.click(within(dialog).getByRole("button", { name: "Select all" }));
  expect(
    within(dialog).getByText("3 Target groups selected", {
      exact: false,
    }),
  ).toBeInTheDocument();
});

test("single-skill Adopt existing hands off to the existing Adopt surface", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const adopted = vi.fn();
  const cell: EnableCell = {
    cellKey: "skill-authoring|fixture-root",
    skillId: "skill-authoring",
    skillName: "Skill authoring",
    directoryName: "skill-authoring",
    directoryIdentityKey: "skill-authoring",
    targetRootId: "fixture-root",
    targetPath: "/Users/me/.claude/skills",
    entryPath: "/Users/me/.claude/skills/skill-authoring",
    finalEntityPath: "/Users/me/Library/skills/skill-authoring",
    action: "enable",
    affectedAgentIds: ["claude-code"],
    affectedAgentNames: ["Claude Code"],
    occupier: {
      untracked: { kind: "real_directory", target: null },
    },
    occExactDirect: false,
    destructive: { directories: 1, files: 0 },
    eligibility: "conflict",
    blockedReason: null,
    resolution: "skip",
    detail: null,
    createSteps: [],
    hopEvidence: [],
  };
  client.planGlobalEnable = async (): Promise<EnablePlan> => ({
    planToken: "adopt-handoff-plan",
    scope: "global",
    writeGateGeneration: 0,
    catalogGeneration: 1,
    agentGeneration: 1,
    projectRoot: null,
    cells: [cell],
  });

  render(
    <GlobalEnableSheet
      client={client}
      skillId="skill-authoring"
      skillName="Skill authoring"
      onAdoptExisting={adopted}
      onClose={vi.fn()}
    />,
  );
  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  await user.click(within(dialog).getByText("Claude Code"));
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );
  await within(dialog).findByRole("combobox");
  await user.selectOptions(within(dialog).getByRole("combobox"), "adopt");
  await user.click(
    within(dialog).getByRole("button", { name: "Open Adopt surface" }),
  );

  expect(adopted).toHaveBeenCalledWith(cell);
});

test("Global Enable refuses dismissal while Apply is in flight", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let resolveApply!: (
    result: Awaited<ReturnType<typeof client.applyGlobalEnable>>,
  ) => void;
  client.applyGlobalEnable = () =>
    new Promise((resolve) => {
      resolveApply = resolve;
    });
  const onClose = vi.fn();
  const { container } = render(
    <GlobalEnableSheet
      client={client}
      skillId="skill-authoring"
      skillName="Skill authoring"
      onClose={onClose}
    />,
  );
  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  await user.click(within(dialog).getByText("Workbench"));
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );
  await within(dialog).findByText("Ready");
  await user.click(
    within(dialog).getByRole("button", { name: "Enable in this Target" }),
  );

  await user.keyboard("{Escape}");
  fireEvent.mouseDown(container.querySelector(".activation-sheet-backdrop")!);
  expect(onClose).not.toHaveBeenCalled();
  expect(within(dialog).getByRole("button", { name: "Back" })).toBeDisabled();

  resolveApply({
    operationId: "fixture-op-apply",
    cells: [],
    snapshotVersion: 1,
  });
  await within(dialog).findByText("0 of 0 cells succeeded");
});
