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
import { App } from "../../app/App";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { expandLibrary } from "../../test-fixtures/expand-library";
import { SkillSelectionStack } from "./SkillSelectionStack";
import type { SkillDetail } from "../../app/catalog-client";

async function setup() {
  const client = createFixtureCatalogClient();
  const inspect = vi.spyOn(client, "inspectSkill");
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await expandLibrary();
  const rows = [...document.querySelectorAll<HTMLButtonElement>(".skill-row")];
  return { client, inspect, rows };
}

test("selection retains detail and target DOM while fresh selection data is pending", async () => {
  const { client, rows } = await setup();
  const panel = screen.getByRole("complementary", {
    name: "Skill distribution",
  });
  const toggle = await within(panel).findByRole("switch", {
    name: "Claude Code",
  });
  const next = await client.inspectSkill("media-xray");
  const authoring = await client.inspectSkill("skill-authoring");
  let resolveDetail!: (value: SkillDetail) => void;
  vi.spyOn(client, "inspectSkill").mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveDetail = resolve;
      }),
  );
  const target = rows.find((row) => row.textContent?.includes("media-xray"))!;
  fireEvent.click(target);
  expect(
    screen.getByRole("complementary", { name: "Skill distribution" }),
  ).toBe(panel);
  expect(toggle).toBeInTheDocument();
  expect(
    screen.getByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();
  await act(async () => resolveDetail(next));
  expect(
    screen.getByRole("heading", { name: "media-xray" }),
  ).toBeInTheDocument();
  fireEvent.click(
    rows.find((row) => row.textContent?.includes("skill-authoring"))!,
    { metaKey: true },
  );
  expect(
    screen.getByRole("complementary", { name: "Skill distribution" }),
  ).toBe(panel);
  expect(toggle).toBeInTheDocument();
  expect(
    screen.getByRole("main", { name: "2 skills selected" }),
  ).toBeInTheDocument();
  fireEvent.click(
    rows.find((row) => row.textContent?.includes("skill-authoring"))!,
  );
  expect(
    screen.getByRole("main", { name: "2 skills selected" }),
  ).toHaveAttribute("inert");
  await act(async () => resolveDetail(authoring));
  expect(
    screen.getByRole("main", { name: "Skill detail" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();
});

test("direct selection supports modifier toggles, empty selection, and a lightweight stack", async () => {
  const { rows, inspect } = await setup();
  expect(
    screen.queryByRole("button", { name: "Batch actions" }),
  ).not.toBeInTheDocument();
  expect(document.querySelector(".skill-select-checkbox")).toBeNull();
  await userEvent.click(rows[0]);
  inspect.mockClear();
  fireEvent.click(rows[1], { metaKey: true });
  const stack = screen.getByRole("main", { name: "2 skills selected" });
  expect(within(stack).getAllByRole("article")).toHaveLength(1);
  expect(document.querySelector(".document-preview")).toBeNull();
  expect(inspect).not.toHaveBeenCalled();
  expect(
    within(
      screen.getByRole("complementary", { name: "Skill distribution" }),
    ).getByRole("button", { name: "Enable to Project…" }),
  ).toBeInTheDocument();
  fireEvent.click(rows[0], { ctrlKey: true });
  expect(rows[0]).toHaveAttribute("aria-pressed", "false");
  expect(rows[1]).toHaveAttribute("aria-pressed", "true");
  await waitFor(() => expect(inspect).toHaveBeenCalledOnce());
  fireEvent.click(rows[1], { metaKey: true });
  expect(
    screen.getByRole("heading", { name: "Select a skill" }),
  ).toBeInTheDocument();
  expect(document.querySelector(".skill-selection-stack")).toBeNull();
  fireEvent.click(rows[2], { shiftKey: true });
  expect(rows.every((row) => row.getAttribute("aria-pressed") === "true")).toBe(
    true,
  );
  expect(
    screen.getByRole("main", { name: "3 skills selected" }),
  ).toBeInTheDocument();
  await userEvent.click(rows[1]);
  expect(
    rows.filter((row) => row.getAttribute("aria-pressed") === "true"),
  ).toEqual([rows[1]]);
});

test("multi-selection project action carries all selected skills into its sheet", async () => {
  const { rows } = await setup();
  fireEvent.click(rows[0]);
  fireEvent.click(rows[1], { metaKey: true });
  const panel = screen.getByRole("complementary", {
    name: "Skill distribution",
  });
  expect(
    within(panel).queryByRole("button", { name: "Remove…" }),
  ).not.toBeInTheDocument();
  await userEvent.click(
    within(panel).getByRole("button", { name: "Enable to Project…" }),
  );
  expect(await screen.findByRole("dialog", { name: /2 skills/ })).toBeVisible();
});

test("Shift uses the selected anchor and Escape clears selection without a batch mode", async () => {
  const { rows } = await setup();
  fireEvent.click(rows[2]);
  fireEvent.click(rows[0], { shiftKey: true });
  expect(rows.every((row) => row.getAttribute("aria-pressed") === "true")).toBe(
    true,
  );
  fireEvent.click(rows[1], { shiftKey: true });
  expect(rows[0]).toHaveAttribute("aria-pressed", "false");
  expect(rows[1]).toHaveAttribute("aria-pressed", "true");
  expect(rows[2]).toHaveAttribute("aria-pressed", "true");
  fireEvent.keyDown(rows[1], { key: "Escape" });
  expect(
    rows.every((row) => row.getAttribute("aria-pressed") === "false"),
  ).toBe(true);
  fireEvent.keyDown(rows[0], { key: "Enter" });
  fireEvent.keyDown(rows[1], { key: " ", ctrlKey: true });
  expect(
    screen.getByRole("main", { name: "2 skills selected" }),
  ).toBeInTheDocument();
});

test("stack DOM stays constant even for a large selection", async () => {
  const client = createFixtureCatalogClient();
  const { items } = await client.listSkills("all");
  const view = render(
    <SkillSelectionStack
      skills={Array.from({ length: 500 }, (_, i) => ({
        ...items[0],
        id: `${i}`,
      }))}
    />,
  );
  expect(view.container.querySelectorAll("article")).toHaveLength(1);
  expect(view.container.querySelectorAll(".skill-stack-back")).toHaveLength(2);
});

test("a late single-detail response cannot replace a multiple selection", async () => {
  const { client, inspect, rows } = await setup();
  const snapshot = await client.listSkills("all");
  const skill = snapshot.items.find(
    (item) => item.directoryName === rows[0].getAttribute("aria-label"),
  )!;
  const detail = await client.inspectSkill(skill.id);
  let resolve!: (detail: SkillDetail) => void;
  inspect.mockImplementationOnce(
    () =>
      new Promise<SkillDetail>((done) => {
        resolve = done;
      }),
  );
  fireEvent.click(rows[0]);
  fireEvent.click(rows[1], { metaKey: true });
  await act(async () => {
    resolve(detail);
  });
  expect(
    screen.getByRole("main", { name: "2 skills selected" }),
  ).toBeInTheDocument();
  expect(document.querySelector(".document-preview")).toBeNull();
  expect(
    within(
      screen.getByRole("complementary", { name: "Skill distribution" }),
    ).getByRole("button", { name: "Enable to Project…" }),
  ).toBeInTheDocument();
});
