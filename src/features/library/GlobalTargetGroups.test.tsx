import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import type {
  GlobalTargetGroupSnapshot,
  ObservationAndScanSnapshot,
} from "../../app/catalog-client";
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
    name: "Activation Target Groups",
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
    name: "Activation Target Groups",
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
    name: "Activation Target Groups",
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
