import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import type {
  GlobalTargetGroupSnapshot,
  ObservationAndScanSnapshot,
} from "../../app/catalog-client";
import { GlobalTargetGroupsPanel } from "./GlobalTargetGroups";
import { BackgroundOperations } from "../../ui/BackgroundOperations";

function renderPanel({
  skillId = "skill-authoring",
  client = createFixtureCatalogClient(),
  skills,
}: {
  skillId?: string;
  client?: ReturnType<typeof createFixtureCatalogClient>;
  skills?: { id: string; name: string }[];
} = {}) {
  return render(
    <BackgroundOperations>
      <GlobalTargetGroupsPanel
        ref={(element) => void element}
        dialog={false}
        skillId={skillId}
        skills={skills}
        client={client}
        onOpenAgentManagement={() => {}}
      />
    </BackgroundOperations>,
  );
}

test("user-configured targets do not display compatibility warnings", async () => {
  renderPanel();
  await screen.findByRole("switch", { name: "Workbench" });
  expect(screen.queryByText("Compatibility unknown")).not.toBeInTheDocument();
});

const batchSkills = [
  { id: "skill-authoring", name: "Skill authoring" },
  { id: "media-xray", name: "Media X-ray" },
];

test("selection refresh keeps cards mounted, blocks stale actions, and ignores late reads", async () => {
  const client = createFixtureCatalogClient();
  const media = await client.listTargetGroups("media-xray");
  const read = client.listTargetGroups.bind(client);
  let resolveMedia!: (snapshot: GlobalTargetGroupSnapshot) => void;
  vi.spyOn(client, "listTargetGroups").mockImplementation((id) =>
    id === "media-xray"
      ? new Promise((resolve) => {
          resolveMedia = resolve;
        })
      : read(id),
  );
  const plan = vi.spyOn(client, "planGlobalLifecycle");
  const view = (id: string) => (
    <BackgroundOperations>
      <GlobalTargetGroupsPanel
        ref={null}
        skillId={id}
        client={client}
        dialog={false}
        onOpenAgentManagement={() => {}}
      />
    </BackgroundOperations>
  );
  const { rerender } = render(view("skill-authoring"));
  const toggle = await screen.findByRole("switch", { name: "Claude Code" });
  const panel = screen.getByRole("complementary");
  rerender(view("media-xray"));
  expect(screen.getByRole("complementary")).toBe(panel);
  expect(screen.getByRole("switch", { name: "Claude Code" })).toBe(toggle);
  expect(toggle).toBeDisabled();
  fireEvent.click(toggle);
  expect(plan).not.toHaveBeenCalled();
  rerender(view("skill-authoring"));
  await waitFor(() => expect(toggle).not.toBeDisabled());
  await act(async () => resolveMedia(media));
  expect(toggle).toBeChecked();
  await userEvent.click(toggle);
  await waitFor(() =>
    expect(plan).toHaveBeenCalledWith(
      "skill-authoring",
      expect.any(String),
      "disable",
    ),
  );
});

test("batch targets distinguish all, none and partial without loading documents", async () => {
  const client = createFixtureCatalogClient();
  const read = client.listTargetGroups.bind(client);
  vi.spyOn(client, "listTargetGroups").mockImplementation(async (id) => {
    const snapshot = await read(id);
    return {
      ...snapshot,
      groups: snapshot.groups.map((group, index) => ({
        ...group,
        desired: index === 0 || (index === 1 && id === batchSkills[0].id),
      })),
    };
  });
  const inspect = vi.spyOn(client, "inspectSkill");
  renderPanel({ client, skills: batchSkills });
  expect(
    await screen.findByRole("switch", { name: "Claude Code" }),
  ).toBeChecked();
  expect(screen.getByRole("switch", { name: "Codex" })).not.toBeChecked();
  expect(screen.getByRole("switch", { name: "Workbench" })).not.toBeChecked();
  expect(screen.getByText("Distributed")).toBeVisible();
  expect(screen.getByText("Not distributed")).toBeVisible();
  expect(screen.getByText("Partially distributed")).toBeVisible();
  expect(inspect).not.toHaveBeenCalled();
});

test("partial target switch previews all selected skills, with no implicit write", async () => {
  const client = createFixtureCatalogClient();
  const plan = vi.spyOn(client, "planGlobalEnable");
  const apply = vi.spyOn(client, "applyGlobalEnable");
  const snapshots = await Promise.all(
    batchSkills.map((skill) => client.listTargetGroups(skill.id)),
  );
  const groupId = snapshots[0].groups[0].targetRootId;
  renderPanel({ client, skills: batchSkills });
  await userEvent.click(
    await screen.findByRole("switch", { name: "Claude Code" }),
  );
  const dialog = await screen.findByRole("dialog", {
    name: "Enable 2 skills globally",
  });
  await userEvent.click(
    await within(dialog).findByRole("button", { name: "Review the plan" }),
  );
  await waitFor(() =>
    expect(plan).toHaveBeenCalledWith(
      batchSkills.map((skill) => skill.id),
      [groupId],
      [],
    ),
  );
  expect(apply).not.toHaveBeenCalled();
});

test("all-enabled target disables only that target for every selected skill", async () => {
  const client = createFixtureCatalogClient();
  const groupId = (await client.listTargetGroups("skill-authoring")).groups[2]
    .targetRootId;
  const initial = await client.planGlobalEnable(
    batchSkills.map((skill) => skill.id),
    [groupId],
    [],
  );
  await client.applyGlobalEnable(initial.planToken);
  const lifecycle = vi.spyOn(client, "planGlobalLifecycle");
  renderPanel({ client, skills: batchSkills });
  const toggle = await screen.findByRole("switch", { name: "Workbench" });
  expect(toggle).toBeChecked();
  await userEvent.click(toggle);
  await waitFor(() => expect(toggle).not.toBeChecked());
  expect(lifecycle.mock.calls).toEqual(
    batchSkills.map((skill) => [skill.id, groupId, "disable"]),
  );
  expect(
    await screen.findByText("Distribution withdrawn for 2 of 2 skills."),
  ).toBeVisible();
});

test("batch disable stops on failure and reports the completed count", async () => {
  const client = createFixtureCatalogClient();
  const groupId = (await client.listTargetGroups("skill-authoring")).groups[2]
    .targetRootId;
  const initial = await client.planGlobalEnable(
    batchSkills.map((skill) => skill.id),
    [groupId],
    [],
  );
  await client.applyGlobalEnable(initial.planToken);
  const lifecycle = client.planGlobalLifecycle.bind(client);
  vi.spyOn(client, "planGlobalLifecycle").mockImplementation(
    (id, target, action) => {
      if (id === "media-xray") return Promise.reject({ code: "plan_stale" });
      return lifecycle(id, target, action);
    },
  );
  renderPanel({ client, skills: batchSkills });
  await userEvent.click(
    await screen.findByRole("switch", { name: "Workbench" }),
  );
  expect(
    await screen.findByText(/Distribution withdrawn for 1 of 2 skills/),
  ).toBeVisible();
  expect(screen.getAllByText("Partially distributed").length).toBeGreaterThan(
    0,
  );
  expect(screen.getByRole("switch", { name: "Workbench" })).not.toBeChecked();
});

test("group cards merge consumers and show one desired state per Target", async () => {
  renderPanel();
  const panel = await screen.findByRole("complementary", {
    name: "Skill distribution",
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
    within(panel).getByText(/Project skills are distributed only/),
  ).toBeInTheDocument();
});

test("toggling an off group enables it and the result window offers Undo", async () => {
  const user = userEvent.setup();
  renderPanel();
  const panel = await screen.findByRole("complementary", {
    name: "Skill distribution",
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
  const notices = await screen.findByRole("region", {
    name: "Current activity",
  });
  expect(within(notices).getByText("Distributed")).toBeVisible();
  expect(notices).toHaveTextContent("Skill authoring");
  expect(notices).toHaveTextContent("Workbench");
  expect(notices).not.toHaveTextContent("review and enable");
  await user.click(workbenchSwitch);
  expect(await within(notices).findByText("Not distributed")).toBeVisible();
  expect(notices).toHaveTextContent("The Skill remains in your Library.");
});

test("refreshes target groups when an observation event arrives", async () => {
  const client = createFixtureCatalogClient();
  let emitObservation:
    ((payload: ObservationAndScanSnapshot) => void) | undefined;
  const list = vi.spyOn(client, "listTargetGroups");
  client.listenObservationChanged = async (callback) => {
    emitObservation = callback;
    return () => {
      emitObservation = undefined;
    };
  };
  renderPanel({ client });
  await screen.findByRole("complementary", {
    name: "Skill distribution",
  });
  const before = list.mock.calls.length;
  const observation = await client.getObservationSnapshot();
  emitObservation?.({
    ...observation,
    activationHealth: {
      generation: 1,
      status: "observed",
      targetGroupCounts: {
        targetGroups: 1,
        observedGroups: 1,
        failedGroups: 0,
        unresponsiveGroups: 0,
        entriesTotal: 1,
        entriesPresent: 1,
        entriesUnhealthy: 0,
        entriesUnknown: 0,
      },
      slow: false,
      diagnostic: null,
    },
  });
  await waitFor(() => expect(list.mock.calls.length).toBeGreaterThan(before));
});

test("mismatch health exposes only its typed Disable action", async () => {
  const client = createFixtureCatalogClient();
  const original = client.listTargetGroups.bind(client);
  client.listTargetGroups = async (skillId) => {
    const snapshot = await original(skillId);
    return {
      ...snapshot,
      skillHealth: "source_snapshot_mismatch",
      groups: snapshot.groups.map((group) => ({
        ...group,
        desired: true,
        observedState: "missing",
      })),
    } satisfies GlobalTargetGroupSnapshot;
  };
  renderPanel({ client });

  const panel = await screen.findByRole("complementary", {
    name: "Skill distribution",
  });
  expect(
    within(panel).queryByRole("switch", { name: "Claude Code" }),
  ).not.toBeInTheDocument();
  expect(
    within(panel).queryByRole("button", { name: "Repair" }),
  ).not.toBeInTheDocument();
  expect(
    within(panel).getAllByRole("button", { name: "Disable this Target" }),
  ).toHaveLength(3);
});

test("non-missing observations do not expose ordinary Repair", async () => {
  const client = createFixtureCatalogClient();
  const original = client.listTargetGroups.bind(client);
  client.listTargetGroups = async (skillId) => {
    const snapshot = await original(skillId);
    return {
      ...snapshot,
      groups: snapshot.groups.map((group, index) => ({
        ...group,
        desired: true,
        observedState: index === 0 ? "occupied" : "target_mismatch",
      })),
    } satisfies GlobalTargetGroupSnapshot;
  };
  renderPanel({ client });

  const panel = await screen.findByRole("complementary", {
    name: "Skill distribution",
  });
  expect(
    within(panel).queryByRole("button", { name: "Repair" }),
  ).not.toBeInTheDocument();
});

test("Broken health hides switches and opens dedicated Disable", async () => {
  const client = createFixtureCatalogClient();
  const original = client.listTargetGroups.bind(client);
  client.listTargetGroups = async (skillId) => {
    const snapshot = await original(skillId);
    return {
      ...snapshot,
      skillHealth: "broken",
      groups: snapshot.groups.map((group) => ({
        ...group,
        desired: true,
        observedState: "dangling",
      })),
    };
  };
  const onOpenBrokenDisable = vi.fn();
  render(
    <GlobalTargetGroupsPanel
      ref={(element) => void element}
      dialog={false}
      skillId="skill-authoring"
      client={client}
      onOpenAgentManagement={() => {}}
      onOpenBrokenDisable={onOpenBrokenDisable}
    />,
  );

  const panel = await screen.findByRole("complementary", {
    name: "Skill distribution",
  });
  expect(within(panel).queryAllByRole("switch")).toHaveLength(0);
  expect(
    within(panel).queryByRole("button", { name: "Repair" }),
  ).not.toBeInTheDocument();
  await userEvent.click(
    within(panel).getAllByRole("button", {
      name: "Disable broken activations…",
    })[0],
  );
  expect(onOpenBrokenDisable).toHaveBeenCalledWith(
    expect.any(HTMLButtonElement),
  );
});
