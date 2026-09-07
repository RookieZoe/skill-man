import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { BatchDisableSheet } from "./BatchDisableSheet";
import "../../styles.css";

test("long batches have a bounded sheet with only the body scrolling", async () => {
  const client = createFixtureCatalogClient();
  const original = (await client.listSkills("all")).items[0];
  const snapshot = await client.listTargetGroups(original.id);
  const skills = Array.from({ length: 30 }, (_, index) => ({
    ...original,
    id: `long-batch-${index}`,
  }));
  client.listTargetGroups = async (skillId) => ({ ...snapshot, skillId });
  render(
    <BatchDisableSheet client={client} skills={skills} onClose={() => {}} />,
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Confirm disable" }),
    ).toBeEnabled(),
  );
  const sheet = screen.getByRole("dialog");
  expect(screen.getAllByRole("listitem")).toHaveLength(
    30 * snapshot.groups.filter((group) => group.desired).length,
  );
  expect(getComputedStyle(sheet.parentElement!).overflow).toBe("hidden");
  expect(getComputedStyle(sheet).maxHeight).toBe("min(680px, 100%)");
  expect(getComputedStyle(sheet).minHeight).toBe("0px");
  expect(
    getComputedStyle(sheet.querySelector(".batch-disable-body")!).overflowY,
  ).toBe("auto");
  expect(getComputedStyle(sheet.querySelector("footer")!).flexShrink).toBe("0");
});

test("mixed selection disables only enabled groups and preserves Library skills", async () => {
  const client = createFixtureCatalogClient();
  const skills = (await client.listSkills("all")).items;
  const snapshots = await Promise.all(
    skills.map((skill) => client.listTargetGroups(skill.id)),
  );
  const expected = snapshots.flatMap((snapshot) =>
    snapshot.groups
      .filter((group) => group.desired)
      .map((group) => [snapshot.skillId, group.targetRootId, "disable"]),
  );
  const plan = vi.spyOn(client, "planGlobalLifecycle");
  const changed = vi.fn();
  render(
    <BatchDisableSheet
      client={client}
      skills={skills}
      onClose={() => {}}
      onCatalogChanged={changed}
    />,
  );
  const button = await screen.findByRole("button", { name: "Confirm disable" });
  await waitFor(() => expect(button).toBeEnabled());
  expect(plan).not.toHaveBeenCalled();
  await userEvent.click(button);
  await waitFor(() => expect(changed).toHaveBeenCalledTimes(1));
  expect(plan.mock.calls).toEqual(expected);
  expect(
    (await client.listSkills("all")).items.map((skill) => skill.id),
  ).toEqual(skills.map((skill) => skill.id));
  for (const skill of skills)
    expect(
      (await client.listTargetGroups(skill.id)).groups.every(
        (group) => !group.desired,
      ),
    ).toBe(true);
});

test("a blocked plan stops the batch without applying unsafe operations", async () => {
  const client = createFixtureCatalogClient();
  const skills = (await client.listSkills("all")).items;
  vi.spyOn(client, "planGlobalLifecycle").mockRejectedValue(
    new Error("Target changed"),
  );
  const apply = vi.spyOn(client, "applyGlobalEnable");
  render(
    <BatchDisableSheet client={client} skills={skills} onClose={() => {}} />,
  );
  const button = screen.getByRole("button", { name: "Confirm disable" });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
  expect(await screen.findByRole("alert")).toBeVisible();
  expect(apply).not.toHaveBeenCalled();
  expect(screen.getByRole("status")).toHaveTextContent("Disabled 0/");
});

test("partial failure keeps completed disables and does not misreport refresh failure as rollback", async () => {
  const client = createFixtureCatalogClient();
  const skills = (await client.listSkills("all")).items;
  const originalPlan = client.planGlobalLifecycle.bind(client);
  let calls = 0;
  vi.spyOn(client, "planGlobalLifecycle").mockImplementation((...args) => {
    if (++calls === 2) return Promise.reject(new Error("Target changed"));
    return originalPlan(...args);
  });
  const apply = vi.spyOn(client, "applyGlobalEnable");
  render(
    <BatchDisableSheet
      client={client}
      skills={skills}
      onClose={() => {}}
      onCatalogChanged={async () => {
        throw new Error("Refresh failed");
      }}
    />,
  );
  const button = screen.getByRole("button", { name: "Confirm disable" });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Close" })).toBeEnabled(),
  );
  expect(apply).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("status")).toHaveTextContent("Disabled 1/");
  expect(screen.getByRole("alert")).toBeVisible();
});
