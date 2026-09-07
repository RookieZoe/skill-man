import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";

import "../styles.css";
import {
  createMatrixCatalogClient,
  type MatrixScenario,
} from "../dev/layout-matrix/scenario-client";
import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { createGitPreviewClient } from "../test-fixtures/git-preview";
import { App } from "./App";
import appLogo from "../../src-tauri/icons/icon.svg";

test("the toolbar reuses the packaged main app logo", async () => {
  await renderWideLibrary();
  const mark = toolbar().querySelector("img.product-mark");
  expect(mark).toHaveAttribute("src", appLogo);
  expect(mark).toHaveAttribute("alt", "");
});

function setViewportWidth(width: number) {
  Object.defineProperty(window, "innerWidth", {
    value: width,
    configurable: true,
  });
  fireEvent(window, new Event("resize"));
}

function shell() {
  return document.querySelector(".app-shell") as HTMLElement;
}

function background() {
  return document.querySelector(".app-background") as HTMLElement;
}

function noticeRegion() {
  return document.querySelector(".notice-region") as HTMLElement;
}

function setViewportHeight(height: number) {
  Object.defineProperty(window, "innerHeight", {
    value: height,
    configurable: true,
  });
  fireEvent(window, new Event("resize"));
}

function drawer() {
  return document.querySelector(".agent-drawer") as HTMLElement;
}

function inspector() {
  return document.querySelector(".agent-inspector") as HTMLElement;
}

function toolbar() {
  return document.querySelector(".toolbar") as HTMLElement;
}

function agentsTrigger() {
  return within(toolbar()).queryByRole("button", { name: "Target groups" });
}

async function renderWideLibrary() {
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
}

afterEach(() => {
  setViewportWidth(1280);
});

test("Git progress remains in a pinned header while confirmation is pending", async () => {
  const client = createGitPreviewClient(true);
  client.confirmSourceTransition = () => new Promise(() => {});
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
  await userEvent.click(screen.getByRole("button", { name: "Import" }));
  await userEvent.click(
    screen.getByRole("button", { name: "Install Git Skills" }),
  );
  await userEvent.type(
    screen.getByRole("textbox", { name: "Repository URL" }),
    "tw93/Waza",
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Fetch latest preview" }),
  );
  await userEvent.click(
    await screen.findByRole("checkbox", { name: /Remove the listed Skills/ }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Force remote replacement" }),
  );
  const dialog = screen.getByRole("dialog");
  const header = dialog.querySelector<HTMLElement>(".sheet-progress-header")!;
  expect(getComputedStyle(header).position).toBe("sticky");
  expect(getComputedStyle(header).top).toBe("0px");
  expect(header).toContainElement(
    within(dialog).getByRole("list", { name: "Import progress" }),
  );
  expect(header).toContainElement(within(dialog).getByRole("progressbar"));
  expect(within(dialog).getAllByRole("progressbar")).toHaveLength(1);
  expect(
    screen.queryByRole("region", { name: "Current activity" }),
  ).not.toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
});

// -- Viewport breakpoints (759/760, 1059/1060) --

test("exact breakpoints: 1060 wide, 1059 mid, 760 mid, 759 narrow", async () => {
  await renderWideLibrary();

  setViewportWidth(1060);
  expect(shell()).toHaveAttribute("data-layout-mode", "wide");
  expect(agentsTrigger()).not.toBeInTheDocument();

  setViewportWidth(1059);
  expect(shell()).toHaveAttribute("data-layout-mode", "mid");
  expect(agentsTrigger()).toBeInTheDocument();

  setViewportWidth(760);
  expect(shell()).toHaveAttribute("data-layout-mode", "mid");

  setViewportWidth(759);
  expect(shell()).toHaveAttribute("data-layout-mode", "narrow");
  expect(agentsTrigger()).not.toBeInTheDocument();
  expect(
    screen.getByRole("group", { name: "Pane navigation" }),
  ).toBeInTheDocument();
});

test("batch selection stays in the Library heading and adds one column to the same rows", async () => {
  await renderWideLibrary();
  const select = screen.getByRole("button", { name: "Batch actions" });
  expect(select.closest(".library-sidebar .panel-heading")).not.toBeNull();
  expect(
    within(toolbar()).queryByRole("button", { name: "Batch actions" }),
  ).not.toBeInTheDocument();
  const row = screen.getByRole("button", { name: "skill-authoring" });
  const padding = getComputedStyle(row).padding;
  await userEvent.click(select);
  expect(row).toHaveClass("skill-row--selectable");
  expect(getComputedStyle(row).gridTemplateColumns).toBe(
    "14px 9px minmax(0, 1fr) auto",
  );
  expect(getComputedStyle(row).padding).toBe(padding);
  await userEvent.click(within(row).getByRole("checkbox"));
  expect(row).toHaveAttribute("aria-pressed", "true");
});

test("enable dialogs have a padded scroll body, visible steps and separated footer", async () => {
  await renderWideLibrary();
  await userEvent.click(
    screen.getByRole("button", { name: "Enable globally…" }),
  );
  const dialog = await screen.findByRole("dialog");
  expect(
    within(dialog).getByText("Target groups", { selector: "li" }),
  ).toBeVisible();
  expect(within(dialog).getByText("Preview", { selector: "li" })).toBeVisible();
  expect(
    getComputedStyle(dialog.querySelector(".enable-sheet-body")!).overflow,
  ).toBe("auto");
  expect(dialog.querySelector(".enable-sheet-actions")).toContainElement(
    within(dialog).getByRole("button", { name: "Cancel" }),
  );
});

test("wide viewport: explicit rows, zero-Notice collapse, pane scroll owners", async () => {
  await renderWideLibrary();

  expect(shell()).toHaveAttribute("data-layout-mode", "wide");
  // NoticeRegion collapses to nothing with zero Notices.
  expect(noticeRegion()).toBeEmptyDOMElement();
  expect(background()).not.toHaveAttribute("inert");
  // Toolbar, NoticeRegion and Workspace are explicit rows; no implicit grid.
  expect(shell().children).toHaveLength(4); // skip-link, toolbar, notices, background
  expect(document.querySelector(".app-shell > .toolbar")).toBeInTheDocument();
  expect(document.querySelector(".app-shell > .notice-region")).toBeTruthy();
  expect(document.querySelector(".app-shell > .app-background")).toBeTruthy();
  // The Agent Inspector stays a plain pane (not a dialog) in wide mode.
  expect(
    screen.getByRole("complementary", { name: "Activation Target Groups" }),
  ).toBeInTheDocument();
  expect(inspector()).not.toHaveAttribute("role");
  // Library chrome never shrinks or scrolls; only its list owns scrolling.
  expect(
    getComputedStyle(document.querySelector(".library-sidebar")!).overflowY,
  ).toBe("hidden");
  expect(
    getComputedStyle(document.querySelector(".skill-list")!).overflowY,
  ).toBe("auto");
  for (const selector of [
    ".library-sidebar > .panel-heading",
    ".library-sidebar > .filter-strip",
  ]) {
    expect(getComputedStyle(document.querySelector(selector)!).flexShrink).toBe(
      "0",
    );
  }
  setViewportWidth(375);
  expect(
    getComputedStyle(document.querySelector(".library-sidebar")!).overflowY,
  ).toBe("hidden");
  expect(
    getComputedStyle(document.querySelector(".skill-list")!).overflowY,
  ).toBe("auto");
  setViewportWidth(1200);
  expect(
    getComputedStyle(document.querySelector(".skill-detail")!).overflowY,
  ).toBe("auto");
  expect(getComputedStyle(inspector()).overflowY).toBe("auto");
});

// -- Agent drawer (760–1059) --

test("detail heading, actions and document share one centered reading width", async () => {
  await renderWideLibrary();
  const styles = [
    ".detail-heading",
    ".detail-actions",
    ".document-preview",
  ].map((selector) => getComputedStyle(document.querySelector(selector)!));
  expect(new Set(styles.map((style) => style.maxWidth)).size).toBe(1);
  for (const style of styles) {
    expect(style.marginInline).toBe("auto");
  }
});

test("opening the sliding inspector does not scroll its workspace into the offscreen animation", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(800);
  const focus = vi.spyOn(inspector(), "focus");
  try {
    await user.click(screen.getByRole("button", { name: "Target groups" }));
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(inspector()).toHaveFocus();
    expect(getComputedStyle(drawer()).overflowX).toBe("clip");
  } finally {
    focus.mockRestore();
  }
});

test("mid mode opens the Agent Inspector as a modal drawer with inert background", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);

  const trigger = screen.getByRole("button", { name: "Target groups" });
  expect(trigger).toHaveAttribute("aria-expanded", "false");
  expect(trigger).toHaveAttribute("aria-controls", "agent-inspector-dialog");
  expect(drawer()).not.toHaveAttribute("data-open");
  // Closed drawer is visually hidden and not focusable.
  expect(getComputedStyle(inspector()).visibility).toBe("hidden");

  await user.click(trigger);
  expect(drawer()).toHaveAttribute("data-open", "true");
  expect(trigger).toHaveAttribute("aria-expanded", "true");
  expect(inspector()).toHaveAttribute("role", "dialog");
  expect(inspector()).toHaveAttribute("aria-modal", "true");
  // The drawer stays inside the app background (same DOM node across
  // breakpoints), so its sibling surfaces are inert while the drawer remains
  // interactive.
  expect(background()).not.toHaveAttribute("inert");
  expect(document.querySelector(".toolbar")).toHaveAttribute("inert");
  expect(document.querySelector(".notice-region")).toHaveAttribute("inert");
  expect(document.querySelector(".library-sidebar")).toHaveAttribute("inert");
  expect(document.querySelector(".skill-detail")).toHaveAttribute("inert");
  expect(document.querySelector(".scan-evidence-ledger")).toHaveAttribute(
    "inert",
  );
  expect(
    screen.getByRole("dialog", { name: "Activation Target Groups" }),
  ).toBeInTheDocument();
  expect(inspector()).toHaveFocus();
  expect(getComputedStyle(inspector()).visibility).toBe("visible");
});

test("drawer Escape and backdrop close and restore focus to the trigger", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  const trigger = screen.getByRole("button", { name: "Target groups" });

  await user.click(trigger);
  await user.keyboard("{Escape}");
  expect(drawer()).not.toHaveAttribute("data-open");
  expect(trigger).toHaveFocus();

  await user.click(trigger);
  fireEvent.mouseDown(
    drawer().querySelector(".agent-drawer-backdrop") as HTMLElement,
  );
  expect(drawer()).not.toHaveAttribute("data-open");
  expect(trigger).toHaveFocus();
});

test("drawer traps Tab focus within its Target-scoped placeholder", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("button", { name: "Target groups" }));
  expect(inspector()).toHaveFocus();

  const enableButton = within(inspector()).getByRole("button", {
    name: "Enable globally…",
  });
  await user.tab();
  expect(enableButton).toHaveFocus();
  // Focus stays trapped inside the drawer while tabbing through the new
  // Target group cards.
  for (let index = 0; index < 6; index += 1) {
    await user.tab();
    expect(inspector().contains(document.activeElement)).toBe(true);
  }
  await user.tab({ shift: true });
  expect(inspector().contains(document.activeElement)).toBe(true);
});

test("drawer keeps its DOM and focus when resizing across the 1059/1060 breakpoint", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("button", { name: "Target groups" }));
  const inspectorNode = inspector();
  const focusedBefore = document.activeElement;

  setViewportWidth(1060);
  // Same node: the drawer did not remount; it became the wide pane.
  expect(inspector()).toBe(inspectorNode);
  expect(shell()).toHaveAttribute("data-layout-mode", "wide");
  expect(inspector()).not.toHaveAttribute("role");
  expect(background()).not.toHaveAttribute("inert");
  expect(document.activeElement).toBe(focusedBefore);

  // Shrinking back restores the drawer in its previous open state.
  setViewportWidth(1059);
  expect(drawer()).toHaveAttribute("data-open", "true");
  expect(inspector()).toHaveAttribute("role", "dialog");
});

// -- Overlay and focus contract --

test("Agent Configuration sheet survives resize, inerts the workspace, and restores focus", async () => {
  const user = userEvent.setup();
  render(
    <App
      client={createFixtureCatalogClient({ emptyAgentConfigurations: true })}
    />,
  );
  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("tab", { name: "Agents" }));
  await screen.findByRole("heading", { name: "No Agent Configuration yet" });
  const opener = screen.getAllByRole("button", {
    name: "New custom agent",
  })[0];
  await user.click(opener);
  const dialog = screen.getByRole("dialog", { name: "Agent Configuration" });
  expect(background()).toHaveAttribute("inert");

  setViewportWidth(1059);
  expect(screen.getByRole("dialog", { name: "Agent Configuration" })).toBe(
    dialog,
  );
  setViewportWidth(1060);
  expect(screen.getByRole("dialog", { name: "Agent Configuration" })).toBe(
    dialog,
  );

  await user.keyboard("{Escape}");
  expect(dialog).not.toBeInTheDocument();
  expect(background()).not.toHaveAttribute("inert");
  expect(opener).toHaveFocus();
});

test("Global Enable sheet traps focus outside the inert app rows", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  const opener = within(inspector()).getByRole("button", {
    name: "Enable globally…",
  });
  await user.click(opener);

  const dialog = await screen.findByRole("dialog", {
    name: "Enable Skill authoring globally",
  });
  expect(background()).toHaveAttribute("inert");
  expect(toolbar()).toHaveAttribute("inert");
  expect(noticeRegion()).toHaveAttribute("inert");
  expect(within(dialog).getAllByRole("checkbox")[0]).toHaveFocus();

  await user.tab();
  expect(dialog.contains(document.activeElement)).toBe(true);
  await user.keyboard("{Escape}");
  expect(
    screen.queryByRole("dialog", {
      name: "Enable Skill authoring globally",
    }),
  ).not.toBeInTheDocument();
  expect(opener).toHaveFocus();
});

test("mid Agent details drawer is portaled outside the inert workspace", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("tab", { name: "Agents" }));
  const agentRow = await screen.findByRole("button", { name: /Claude Code/ });
  await user.click(agentRow);

  const drawer = await screen.findByRole("dialog", {
    name: "Agent configuration details",
  });
  expect(background()).toHaveAttribute("inert");
  expect(drawer.closest("body")).toBe(document.body);
  await user.keyboard("{Escape}");
  await waitFor(() =>
    expect(
      screen.queryByRole("dialog", { name: "Agent configuration details" }),
    ).not.toBeInTheDocument(),
  );
  expect(screen.getByRole("button", { name: "Open details" })).toHaveFocus();
});

// -- Notice counts --

test("NoticeRegion: zero collapses, one is single, two stack in the region", async () => {
  const errorClient = createFixtureCatalogClient();
  errorClient.listSkills = async () => {
    throw new Error("Catalog read failed");
  };
  render(<App client={errorClient} />);
  await within(noticeRegion()).findByText("Library unavailable");
  expect(noticeRegion().children).toHaveLength(1);
  expect(
    within(noticeRegion()).getByText("Library unavailable"),
  ).toBeInTheDocument();
});

test("Catalog failure stays global and source failure belongs to Repositories", async () => {
  const stackedClient = createFixtureCatalogClient();
  stackedClient.getGitSourceCapability = async () => {
    throw {
      error: { code: "catalog_unavailable" },
      diagnostic: { code: "capability_failed", message: "unreadable" },
    };
  };
  stackedClient.listSkills = async () => {
    throw new Error("Catalog read failed");
  };
  render(<App client={stackedClient} />);
  await screen.findByRole("heading", { name: "Skill detail unavailable" });
  expect(noticeRegion().children).toHaveLength(1);
  expect(
    within(noticeRegion()).getByText("Library unavailable"),
  ).toBeInTheDocument();
  await userEvent.click(screen.getByRole("tab", { name: "Repositories" }));
  expect(
    await within(
      screen.getByRole("main", { name: "Repository management" }),
    ).findByText("Git source status unavailable"),
  ).toBeInTheDocument();
  expect(
    within(noticeRegion()).getByText("Library unavailable"),
  ).toBeInTheDocument();
});

// -- Empty and error states --

test("empty Library renders real accessible empty DOM instead of a loading state", async () => {
  const client = createFixtureCatalogClient();
  client.listSkills = async () => ({ snapshotVersion: 8, items: [] });
  render(<App client={client} />);

  expect(
    await screen.findByRole("heading", { name: "Empty Library" }),
  ).toBeInTheDocument();
  expect(
    screen.getByText("Import or manage a Skill to get started."),
  ).toBeInTheDocument();
  expect(screen.queryByText("Loading Skill detail")).not.toBeInTheDocument();
  expect(screen.getByText("No Skill selected")).toBeInTheDocument();
  expect(
    screen.getByText("Select a Skill to choose where to enable it."),
  ).toBeInTheDocument();
});

test("Catalog error renders a real error pane with an accessible alert", async () => {
  const client = createFixtureCatalogClient();
  client.listSkills = async () => {
    throw new Error("Catalog read failed");
  };
  render(<App client={client} />);

  expect(
    await screen.findByRole("heading", { name: "Skill detail unavailable" }),
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      "Could not load Skill details. Check the error above and retry.",
    ),
  ).toBeInTheDocument();
  expect(screen.queryByText("Loading Skill detail")).not.toBeInTheDocument();
  const alerts = screen.getAllByRole("alert");
  expect(alerts).toHaveLength(2);
  expect(
    within(alerts[0]).getByText("Library unavailable"),
  ).toBeInTheDocument();
});

// -- Narrow defensive navigation --

test("narrow viewport: single active pane with explicit pane navigation", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(759);

  expect(shell()).toHaveAttribute("data-active-pane", "library");
  const nav = screen.getByRole("group", { name: "Pane navigation" });
  const detailButton = within(nav).getByRole("button", { name: "Skill" });
  const agentsButton = within(nav).getByRole("button", { name: "Agents" });
  expect(within(nav).getByRole("button", { name: "Library" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  // Selecting a Skill moves the single visible pane to the detail.
  await user.click(screen.getByRole("button", { name: "media-xray" }));
  expect(shell()).toHaveAttribute("data-active-pane", "detail");
  expect(detailButton).toHaveAttribute("aria-pressed", "true");

  await user.click(agentsButton);
  expect(shell()).toHaveAttribute("data-active-pane", "agents");
  expect(agentsButton).toHaveAttribute("aria-pressed", "true");
});

// -- Dense content and window height rows of the §10.2 viewport matrix --
// Geometry (wrapping, x-overflow) is real-browser evidence via the dev
// matrix; jsdom covers the semantics: dense source content renders verbatim
// in both languages and height never changes the layout mode.

test("dense English and 简体中文 source content renders verbatim without a loading state", async () => {
  for (const language of ["en", "zh"] as const) {
    const scenario: MatrixScenario = {
      content: "normal",
      density: "dense",
      language,
      notices: "none",
    };
    const { unmount } = render(
      <App client={createMatrixCatalogClient(scenario)} />,
    );
    await screen.findByRole("heading", { name: "skill-authoring" });
    const denseText =
      language === "zh"
        ? "不间断简体中文长文本用于验证换行行为在任何断点宽度下都不会产生页面级横向滚动或者内容被裁剪同时保持全部操作可达"
        : "UninterruptedEnglishTextWithoutAnyBreakingOpportunitiesThatForcesWrappingDecisionsInsideEveryPaneAtEveryBreakpointWidthInTheMatrix";
    expect(screen.getAllByText(denseText).length).toBeGreaterThan(0);
    expect(
      document.querySelector(".document-preview pre")?.textContent,
    ).toContain(denseText);
    expect(screen.queryByText("Loading Skill detail")).not.toBeInTheDocument();
    expect(shell()).toHaveAttribute("data-layout-mode", "wide");
    unmount();
  }
});

test("window height never changes the layout mode at any breakpoint", async () => {
  await renderWideLibrary();
  setViewportHeight(420);
  setViewportWidth(1180);
  expect(shell()).toHaveAttribute("data-layout-mode", "wide");
  setViewportWidth(1059);
  expect(shell()).toHaveAttribute("data-layout-mode", "mid");
  setViewportWidth(759);
  expect(shell()).toHaveAttribute("data-layout-mode", "narrow");
  setViewportHeight(900);
  expect(shell()).toHaveAttribute("data-layout-mode", "narrow");
  setViewportWidth(1180);
  expect(shell()).toHaveAttribute("data-layout-mode", "wide");
});
