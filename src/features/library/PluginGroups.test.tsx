import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { groupPluginMembers, PluginGroups } from "./PluginGroups";

test("groups plugin members before Other without inventing categories", () => {
  const items = [
    { id: "retro" },
    { id: "review", pluginName: "mattpocock-skills" },
  ];
  expect(
    groupPluginMembers(items, (m) => m.pluginName)?.map(([name]) => name),
  ).toEqual(["mattpocock-skills", ""]);
  expect(groupPluginMembers([items[0]], (m) => m.pluginName)).toBeNull();
});

test("Library categories are static headings with visible members", () => {
  const { container } = render(
    <PluginGroups
      items={[{ name: "waza-ui" }]}
      pluginName={(m) => m.name}
      collapsible={false}
    >
      {() => <button>ui</button>}
    </PluginGroups>,
  );
  expect(screen.getByRole("heading", { name: /Waza Ui/ })).toBeVisible();
  expect(screen.getByRole("button", { name: "ui" })).toBeVisible();
  expect(container.querySelector("details")).toBeNull();
});

test("category disclosure hides members without changing their selection", async () => {
  const user = userEvent.setup();
  render(
    <PluginGroups
      items={[
        { id: "review", pluginName: "mattpocock-skills" },
        { id: "retro", pluginName: null },
      ]}
      pluginName={(m) => m.pluginName}
    >
      {(members) => (
        <ul>
          {members.map((m) => (
            <li key={m.id}>
              <input type="checkbox" aria-label={m.id} defaultChecked />
            </li>
          ))}
        </ul>
      )}
    </PluginGroups>,
  );
  const heading = screen.getByText("Mattpocock Skills");
  expect(screen.getByText("Other")).toBeInTheDocument();
  expect(screen.getByRole("checkbox", { name: "review" })).toBeChecked();
  await user.click(heading);
  expect(heading.closest("details")).not.toHaveAttribute("open");
  await user.click(heading);
  expect(screen.getByRole("checkbox", { name: "review" })).toBeChecked();
});
