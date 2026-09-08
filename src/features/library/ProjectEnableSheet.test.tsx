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
  const reviewBtn = within(dialog).getByRole("button", {
    name: "Review the plan",
  });
  await user.click(reviewBtn);

  // Step 3: Preview
  expect(
    (await within(dialog).findAllByText(/skill-authoring/)).length,
  ).toBeGreaterThan(0);
  expect(
    within(dialog).getAllByText(/The project owns this copy/).length,
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
      "Project copies are managed by the project; Skill Man does not track or health-monitor them.",
    ),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByText("1 / 1 actions completed"),
  ).toBeInTheDocument();
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

test("batch project copy scope is explicitly unavailable", async () => {
  const user = userEvent.setup();
  render(
    <ProjectEnableSheet
      client={createFixtureCatalogClient()}
      skills={[
        { id: "a", name: "A" },
        { id: "b", name: "B" },
      ]}
      onClose={() => {}}
    />,
  );
  await user.type(screen.getByPlaceholderText("/path/to/project"), "/project");
  expect(
    screen.getByText(/Multiple Skills are not available yet/),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Review the plan" }),
  ).toBeDisabled();
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

test("occupied entries cannot be replaced before overwrite support", async () => {
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
    within(dialog).getByRole("button", { name: "Review the plan" }),
  );

  // In preview step:
  expect(await within(dialog).findByText("Conflict")).toBeInTheDocument();
  expect(within(dialog).queryByRole("combobox")).not.toBeInTheDocument();
  expect(
    within(dialog).getByText(/This entry is occupied and will be preserved/),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByRole("button", { name: "Enable in Project" }),
  ).toBeDisabled();
});

test("zero additional Agents can preview without General configuration", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const original = client.getAgentManagementSnapshot;
  client.getAgentManagementSnapshot = async () => ({
    ...(await original()),
    configurations: [],
  });
  client.planProjectEnable = async (ids, folder, agents) => {
    expect(ids).toEqual(["skill-authoring"]);
    expect(agents).toEqual([]);
    return {
      planToken: "copy-plan",
      scope: "project",
      writeGateGeneration: 0,
      catalogGeneration: 0,
      agentGeneration: 0,
      projectRoot: null,
      cells: [],
    };
  };
  renderSheet(client);
  await user.type(
    screen.getByPlaceholderText("/path/to/project"),
    "/my/project",
  );
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  expect(
    await screen.findByText(/Required base directory/),
  ).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  expect(
    await screen.findByRole("button", { name: "Enable in Project" }),
  ).toBeInTheDocument();
});

test("Chinese copy-only preview explains reuse and remains executable", async () => {
  const { LocaleProvider } = await import("../locale/LocaleProvider");
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  await client.setLocaleSelection("zh-Hans");
  const original = client.planProjectEnable;
  client.planProjectEnable = async (...args) => {
    const plan = await original(...args);
    return {
      ...plan,
      cells: plan.cells.map((cell) => ({
        ...cell,
        projectCopy: "reuse" as const,
        eligibility: "no_op" as const,
        createSteps: [],
      })),
    };
  };
  render(
    <LocaleProvider client={client}>
      <ProjectEnableSheet
        client={client}
        skillId="skill-authoring"
        skillName="Skill authoring"
        onClose={() => {}}
      />
    </LocaleProvider>,
  );
  await screen.findByText(/必须的基础目录/);
  await user.type(
    screen.getByPlaceholderText("/path/to/project"),
    "/my/project",
  );
  const continueText = (await import("../../../resources/locales/zh-Hans.json"))
    .default["enable.project.continue"];
  await user.click(screen.getByRole("button", { name: continueText }));
  await user.click(screen.getByRole("button", { name: continueText }));
  expect(
    await screen.findByText(
      "使用项目已有内容，不复制 Library 内容，也不覆盖项目副本。",
    ),
  ).toBeInTheDocument();
  const applyText = (await import("../../../resources/locales/zh-Hans.json"))
    .default["enable.project.applyPlan"];
  expect(screen.getByRole("button", { name: applyText })).not.toBeDisabled();
});

test("multiple Agents preview a required copy and dependent links", async () => {
  const user = userEvent.setup();
  renderSheet();
  await user.type(screen.getByPlaceholderText("/path/to/project"), "/project");
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  await user.click(await screen.findByRole("button", { name: "Select all" }));
  expect(screen.getByRole("button", { name: "Review the plan" })).toBeEnabled();
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  expect(
    await screen.findByText(/Skill authoring → Copy from Library/),
  ).toBeInTheDocument();
  expect(screen.getAllByText(/Share project copy with/).length).toBeGreaterThan(
    0,
  );
});

test("partial results identify each Agent, retain Undo feedback, and retry via a fresh preview", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const originalPlan = client.planProjectEnable;
  let previews = 0;
  let captured: Awaited<ReturnType<typeof originalPlan>> | undefined;
  client.planProjectEnable = async (...args) => {
    previews += 1;
    captured = await originalPlan(...args);
    return captured;
  };
  client.applyProjectEnable = async () => ({
    operationId: "partial-operation",
    snapshotVersion: 1,
    cells: captured!.cells.map((cell) => ({
      projectCopy: cell.projectCopy,
      dependsOnCopy: cell.dependsOnCopy,
      cellKey: cell.cellKey,
      skillId: cell.skillId,
      targetRootId: cell.targetRootId,
      outcome: cell.dependsOnCopy ? "failed" : "succeeded",
      diagnostic: null,
    })),
  });
  client.undoProjectEnable = async () => ({
    operationId: "partial-operation",
    snapshotVersion: 1,
    cells: [
      {
        cellKey: captured!.cells[0].cellKey,
        undone: false,
        diagnostic: "preserved",
      },
    ],
  });
  renderSheet(client);
  await user.type(screen.getByPlaceholderText("/path/to/project"), "/project");
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  await user.click(await screen.findByRole("button", { name: "Select all" }));
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  await user.click(
    await screen.findByRole("button", { name: "Enable in Project" }),
  );
  expect(
    await screen.findByText("1 / 2 actions completed"),
  ).toBeInTheDocument();
  expect(
    screen.getByText("Share project copy with Claude Code"),
  ).toBeInTheDocument();
  expect(screen.getByText("Failed")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Undo this operation" }));
  expect(await screen.findByText(/Undo was partial/)).toBeInTheDocument();
  await user.click(
    screen.getByRole("button", { name: "Create a new preview" }),
  );
  await user.click(screen.getByRole("button", { name: "Review the plan" }));
  await screen.findByRole("button", { name: "Enable in Project" });
  expect(previews).toBe(2);
});
