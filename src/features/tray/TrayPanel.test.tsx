import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import { TrayPanel } from "./TrayPanel";

test("searches the whole Library by normalized name, directory and description", async () => {
  const client = createFixtureCatalogClient();
  const { items } = await client.listSkills("all");
  client.listSkills = async () => ({
    snapshotVersion: 1,
    items: Array.from({ length: 8 }, (_, index) => ({
      ...items[0],
      id: String(index),
      displayName: index === 7 ? "Café" : `Skill ${index}`,
      directoryName: `directory-${index}`,
      description: index === 7 ? "Build DOCUMENTS" : "",
    })),
  });
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox");
  expect(await screen.findAllByRole("option")).toHaveLength(8);
  expect(search).toHaveFocus();
  for (const query of ["  CAFE\u0301  ", "DIRECTORY-7", "documents"]) {
    fireEvent.change(search, { target: { value: query } });
    expect(screen.getAllByRole("option")).toHaveLength(1);
    expect(screen.getByRole("option")).toHaveTextContent("Café");
  }
  fireEvent.change(search, { target: { value: "not here" } });
  expect(screen.queryByRole("option")).not.toBeInTheDocument();
  expect(screen.getByText("No matching skills")).toBeVisible();
});

test("Home becoming unavailable discards a pending document and does not present an empty Library", async () => {
  const client = createFixtureCatalogClient();
  const document = await client.inspectSkill("skill-authoring");
  let finish!: (value: typeof document) => void;
  client.inspectSkill = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  let changed!: Parameters<typeof client.listenBootstrapChanged>[0];
  client.listenBootstrapChanged = async (callback) => {
    changed = callback;
    return () => {};
  };
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox");
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "skill-authoring" } });
  fireEvent.keyDown(search, { key: "Enter" });
  await act(async () =>
    changed({
      snapshot: {
        state: "home_unavailable",
        homeId: "home",
        path: "/missing",
        diagnostic: null,
      },
      generation: 2,
    }),
  );
  await act(async () => finish(document));
  expect(screen.getByText("Home unavailable")).toBeVisible();
  expect(screen.queryByText("No matching skills")).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Copy Markdown" }),
  ).not.toBeInTheDocument();
});

test("reads raw Markdown, preserves the search on Back, and ignores IME confirmation", async () => {
  const client = createFixtureCatalogClient();
  const inspect = vi.spyOn(client, "inspectSkill");
  const copyMarkdown = vi.fn().mockResolvedValue(undefined);
  const close = vi.fn();
  render(
    <TrayPanel
      client={client}
      onClose={close}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
      copyMarkdown={copyMarkdown}
    />,
  );
  const search = await screen.findByRole("searchbox");
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "skill-authoring" } });
  fireEvent.keyDown(search, { key: "Enter", isComposing: true });
  expect(inspect).not.toHaveBeenCalled();
  fireEvent.keyDown(search, { key: "Enter" });
  await screen.findByRole("button", { name: "Copy Markdown" });
  fireEvent.click(screen.getByRole("button", { name: "Copy Markdown" }));
  expect(await screen.findByText("Copied")).toBeVisible();
  expect(copyMarkdown).toHaveBeenCalledWith(
    (await client.inspectSkill("skill-authoring")).skillMarkdown,
  );
  fireEvent.keyDown(screen.getByRole("main"), { key: "Escape" });
  expect(screen.getByRole("searchbox")).toHaveValue("skill-authoring");
  expect(screen.getByRole("searchbox")).toHaveFocus();
  fireEvent.keyDown(screen.getByRole("searchbox"), { key: "Escape" });
  expect(close).toHaveBeenCalledOnce();
});

test("a Catalog change invalidates the reading view when its Skill was removed", async () => {
  const client = createFixtureCatalogClient();
  let changed!: () => void;
  client.listenCatalogChanged = async (callback) => {
    changed = callback;
    return () => {};
  };
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox");
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "skill-authoring" } });
  fireEvent.keyDown(search, { key: "Enter" });
  await screen.findByRole("button", { name: "Copy Markdown" });
  client.listSkills = async () => ({ snapshotVersion: 2, items: [] });
  await act(async () => changed());
  expect(
    await screen.findByText(
      "Skill unavailable or removed. Return to the list to continue.",
    ),
  ).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "Copy Markdown" }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  expect(screen.getByText("Your Library is empty")).toBeVisible();
});

test("a late list from the old Home cannot replace the new Home", async () => {
  const client = createFixtureCatalogClient();
  const old = await client.listSkills("all");
  let finish!: (value: typeof old) => void;
  let changed!: Parameters<typeof client.listenBootstrapChanged>[0];
  client.listenBootstrapChanged = async (callback) => {
    changed = callback;
    return () => {};
  };
  client.listSkills = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValue({ snapshotVersion: 2, items: [] });
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  await waitFor(() => expect(finish).toBeDefined());
  await act(async () =>
    changed({
      generation: 2,
      snapshot: {
        state: "bound",
        homeId: "second",
        catalogAccess: "read_write",
        catalogReadonlyReason: null,
        snapshotVersion: 2,
      },
    }),
  );
  await screen.findByText("Your Library is empty");
  await act(async () => finish(old));
  expect(screen.queryByRole("option")).not.toBeInTheDocument();
});

test("Command K returns to search, copy failures are visible, and closing starts a fresh session", async () => {
  const client = createFixtureCatalogClient();
  const props = {
    client,
    onClose: vi.fn(),
    onOpenMain: vi.fn(),
    onQuit: vi.fn(),
    copyMarkdown: vi.fn().mockRejectedValue(new Error("clipboard denied")),
  };
  const view = render(<TrayPanel {...props} />);
  const search = await screen.findByRole("searchbox");
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "skill-authoring" } });
  fireEvent.keyDown(search, { key: "Enter" });
  fireEvent.click(await screen.findByRole("button", { name: "Copy Markdown" }));
  expect(await screen.findByText("Could not copy Markdown")).toBeVisible();
  fireEvent.keyDown(screen.getByRole("main"), { key: "k", metaKey: true });
  expect(screen.getByRole("searchbox")).toHaveFocus();
  expect(screen.getByRole("searchbox")).toHaveValue("skill-authoring");
  view.unmount();
  render(<TrayPanel {...props} />);
  expect(await screen.findByRole("searchbox")).toHaveValue("");
});

test("the first frame and live locale follow the authority while preserving Source Content", async () => {
  const client = createFixtureCatalogClient();
  await client.setLocaleSelection("zh-Hans");
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox", { name: "搜索技能" });
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "skill-authoring" } });
  await act(async () => {
    await client.setLocaleSelection("en");
  });
  expect(screen.getByRole("searchbox", { name: "Search skills" })).toHaveValue(
    "skill-authoring",
  );
  expect(screen.getByRole("option")).toHaveTextContent("Skill authoring");
});

test("same-name skills are ordered by stable identity and show distinct source locations", async () => {
  const client = createFixtureCatalogClient();
  const detail = await client.inspectSkill("skill-authoring");
  client.listSkills = async () => ({
    snapshotVersion: 1,
    items: ["b", "a"].map((id) => ({
      ...detail,
      id,
      displayName: "Same name",
    })),
  });
  client.inspectSkill = async (id) => ({
    ...detail,
    id,
    finalEntityPath: `/source/${id}/skill`,
  });
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  expect(await screen.findByText("/source/a/skill")).toBeVisible();
  expect(screen.getAllByRole("option")[0]).toHaveTextContent("/source/a/skill");
  expect(await screen.findByText("/source/b/skill")).toBeVisible();
});

test("a delayed initial locale snapshot cannot overwrite a newer locale event", async () => {
  const client = createFixtureCatalogClient();
  const old = await client.getLocaleSnapshot();
  let finish!: (value: typeof old) => void;
  client.getLocaleSnapshot = () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  await act(async () => {
    await client.setLocaleSelection("zh-Hans");
  });
  await screen.findByRole("searchbox", { name: "搜索技能" });
  await act(async () => finish(old));
  expect(screen.getByRole("searchbox", { name: "搜索技能" })).toBeVisible();
});

test("Command K focuses search from the list footer without changing the query", async () => {
  const client = createFixtureCatalogClient();
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox");
  fireEvent.change(search, { target: { value: "author" } });
  screen.getByRole("button", { name: "Quit" }).focus();
  fireEvent.keyDown(screen.getByRole("button", { name: "Quit" }), {
    key: "k",
    metaKey: true,
  });
  expect(search).toHaveFocus();
  expect(search).toHaveValue("author");
});

test("returning from reading retains selection and list scroll", async () => {
  const client = createFixtureCatalogClient();
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  await screen.findAllByRole("option");
  const list = screen.getByRole("listbox");
  list.scrollTop = 96;
  fireEvent.scroll(list);
  const row = screen.getAllByRole("option")[1];
  const name = row.textContent;
  fireEvent.click(row);
  await screen.findByRole("button", { name: "Copy Markdown" });
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  expect(screen.getByRole("listbox").scrollTop).toBe(96);
  expect(screen.getByRole("option", { selected: true }).textContent).toBe(name);
});

test("no-result Enter does nothing and opening the panel never starts background work", async () => {
  const client = createFixtureCatalogClient();
  const inspect = vi.spyOn(client, "inspectSkill");
  const rescan = vi.spyOn(client, "startRescan");
  const update = vi.spyOn(client, "checkSkillUpdates");
  const appUpdate = vi.spyOn(client, "checkAppUpdate");
  render(
    <TrayPanel
      client={client}
      onClose={vi.fn()}
      onOpenMain={vi.fn()}
      onQuit={vi.fn()}
    />,
  );
  const search = await screen.findByRole("searchbox");
  await screen.findAllByRole("option");
  fireEvent.change(search, { target: { value: "no matching source" } });
  fireEvent.keyDown(search, { key: "Enter" });
  expect(inspect).not.toHaveBeenCalled();
  expect(rescan).not.toHaveBeenCalled();
  expect(update).not.toHaveBeenCalled();
  expect(appUpdate).not.toHaveBeenCalled();
});
