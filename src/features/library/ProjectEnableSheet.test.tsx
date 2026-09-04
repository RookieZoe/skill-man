import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { ProjectEnableSheet } from "./ProjectEnableSheet";

function renderSheet(client = createFixtureCatalogClient()) {
  return render(
    <ProjectEnableSheet
      client={client}
      skillId="skill-authoring"
      skillName="Skill authoring"
      onClose={() => {}}
    />,
  );
}

test("four-step flow: folder -> agents -> preview -> result with undo", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  renderSheet(client);

  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring to Project",
  });

  // Step 1: Folder
  const folderInput = within(dialog).getByPlaceholderText("/path/to/project");
  await user.type(folderInput, "/my/project");
  expect(folderInput).toHaveValue("/my/project");

  const continueBtn = within(dialog).getByRole("button", {
    name: "Review the plan",
  });
  expect(continueBtn).not.toBeDisabled();
  await user.click(continueBtn);

  // Step 2: Agents
  expect(
    await within(dialog).findByText("No agent selected"),
  ).toBeInTheDocument();
  const selectAllBtn = within(dialog).getByRole("button", {
    name: "Select all",
  });
  await user.click(selectAllBtn);
  expect(
    within(dialog).queryByText("No agent selected"),
  ).not.toBeInTheDocument();

  const reviewBtn = within(dialog).getByRole("button", {
    name: "Review the plan",
  });
  await user.click(reviewBtn);

  // Step 3: Preview
  expect(
    (await within(dialog).findAllByText("Resolved target group")).length,
  ).toBeGreaterThan(0);
  expect(
    within(dialog).getAllByText(/1 physical write/).length,
  ).toBeGreaterThan(0);
  expect(within(dialog).getAllByText("Ready").length).toBeGreaterThan(0);

  const applyBtn = within(dialog).getByRole("button", {
    name: "Enable in Project",
  });
  expect(applyBtn).not.toBeDisabled();
  await user.click(applyBtn);

  // Step 4: Result
  expect(
    await within(dialog).findByText(
      "No Project record was created. Project links are not tracked or monitored.",
    ),
  ).toBeInTheDocument();
  expect(within(dialog).getByText(/cells succeeded/)).toBeInTheDocument();
  expect(
    within(dialog).getByRole("button", { name: "Undo this operation" }),
  ).toBeInTheDocument();

  // Undo
  await user.click(
    within(dialog).getByRole("button", { name: "Undo this operation" }),
  );
  await waitFor(() => {
    expect(
      within(dialog).getByPlaceholderText("/path/to/project"),
    ).toBeInTheDocument();
  });
});

test("folder step renders browse button and input", async () => {
  const client = createFixtureCatalogClient();
  renderSheet(client);

  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring to Project",
  });

  expect(
    within(dialog).getByRole("button", { name: "Browse…" }),
  ).toBeInTheDocument();
  const folderInput = within(dialog).getByPlaceholderText("/path/to/project");
  expect(folderInput).toHaveValue("");
});

test("recent project folders candidate selection and clear", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient({
    recentProjectFolders: [
      {
        canonicalPathKey: "/projects/demo",
        canonicalPath: "/projects/demo",
        lastUsedAt: "2026-08-31T00:00:00Z",
      },
    ],
  });
  renderSheet(client);

  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring to Project",
  });

  // Recent folders displayed
  expect(
    await within(dialog).findByText("Recent project folders"),
  ).toBeInTheDocument();
  const clearBtn = within(dialog).getByRole("button", { name: "Clear recent" });
  await user.click(clearBtn);

  await waitFor(() => {
    expect(
      within(dialog).queryByText("Recent project folders"),
    ).not.toBeInTheDocument();
  });
});

test("destructive ack required for real directory replacement", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();

  // Mock planProjectEnable to return a cell with conflict and destructive count
  client.planProjectEnable = async () => ({
    planToken: "test-token",
    scope: "project",
    writeGateGeneration: 0,
    catalogGeneration: 1,
    agentGeneration: 1,
    cells: [
      {
        cellKey: "skill-authoring|project:/path/.skills",
        skillId: "skill-authoring",
        skillName: "Skill authoring",
        directoryName: "skill-authoring",
        directoryIdentityKey: "skill-authoring",
        targetRootId: "project:/path/.skills",
        targetPath: "/path/.skills",
        entryPath: "/path/.skills/skill-authoring",
        finalEntityPath: "/path/entity",
        action: "enable",
        affectedAgentIds: ["agent-1"],
        affectedAgentNames: ["Agent 1"],
        occupier: { untracked: { kind: "real_directory", target: null } },
        occExactDirect: false,
        destructive: { directories: 2, files: 5 },
        eligibility: "conflict",
        blockedReason: null,
        resolution: "replace",
        detail: null,
        createSteps: [],
        hopEvidence: [],
      },
    ],
  });

  renderSheet(client);
  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring to Project",
  });

  // Step 1 -> Step 2 -> Step 3
  await user.type(
    within(dialog).getByPlaceholderText("/path/to/project"),
    "/my/project",
  );
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Select all" }),
  );
  await user.click(
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );

  // In preview step:
  expect(await within(dialog).findByText("Conflict")).toBeInTheDocument();
  const ackCheckbox = within(dialog).getByRole("checkbox", {
    name: "I understand that replacing this real directory will move its contents to backup.",
  });
  expect(ackCheckbox).not.toBeChecked();

  // Enable button must be disabled until ack is checked
  const applyBtn = within(dialog).getByRole("button", {
    name: "Enable in Project",
  });
  expect(applyBtn).toBeDisabled();

  // Check the ack
  await user.click(ackCheckbox);
  expect(ackCheckbox).toBeChecked();
  expect(applyBtn).not.toBeDisabled();
});
