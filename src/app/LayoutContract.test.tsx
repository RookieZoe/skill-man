import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test } from "vitest";

import "../styles.css";
import {
  createMatrixCatalogClient,
  type MatrixScenario,
} from "../dev/layout-matrix/scenario-client";
import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { App } from "./App";

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
  return within(toolbar()).queryByRole("button", { name: "Agents" });
}

async function renderWideLibrary() {
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });
}

afterEach(() => {
  setViewportWidth(1280);
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
    screen.getByRole("complementary", { name: "Enable by Agent" }),
  ).toBeInTheDocument();
  expect(inspector()).not.toHaveAttribute("role");
  // Each pane owns its vertical scroll.
  expect(
    getComputedStyle(document.querySelector(".library-sidebar")!).overflowY,
  ).toBe("auto");
  expect(
    getComputedStyle(document.querySelector(".skill-detail")!).overflowY,
  ).toBe("auto");
  expect(getComputedStyle(inspector()).overflowY).toBe("auto");
});

// -- Agent drawer (760–1059) --

test("mid mode opens the Agent Inspector as a modal drawer with inert background", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);

  const trigger = screen.getByRole("button", { name: "Agents" });
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
  // breakpoints), so modality comes from the focus trap, backdrop and
  // aria-modal rather than the inert attribute; sheets own the inert gate.
  expect(background()).not.toHaveAttribute("inert");
  expect(
    screen.getByRole("dialog", { name: "Enable by Agent" }),
  ).toBeInTheDocument();
  expect(inspector()).toHaveFocus();
  expect(getComputedStyle(inspector()).visibility).toBe("visible");
});

test("drawer Escape and backdrop close and restore focus to the trigger", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  const trigger = screen.getByRole("button", { name: "Agents" });

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

test("drawer traps Tab focus within its activation controls", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("button", { name: "Agents" }));
  expect(inspector()).toHaveFocus();

  const claudeSwitch = screen.getByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });
  const workbenchSwitch = screen.getByRole("switch", {
    name: "Enable skill-authoring for Workbench",
  });
  await user.tab();
  expect(claudeSwitch).toHaveFocus();
  // Wrap forward from the last focusable back to the first.
  await user.tab();
  await user.tab();
  await user.tab();
  await user.tab();
  expect(claudeSwitch).toHaveFocus();
  // Wrap backward from the first focusable to the last.
  await user.tab({ shift: true });
  expect(workbenchSwitch).toHaveFocus();
});

test("drawer keeps its DOM and focus when resizing across the 1059/1060 breakpoint", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("button", { name: "Agents" }));
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

test("activation sheet survives a breakpoint resize and restores its opener on close", async () => {
  const user = userEvent.setup();
  await renderWideLibrary();
  setViewportWidth(1059);
  await user.click(screen.getByRole("button", { name: "Agents" }));
  const activationSwitch = screen.getByRole("switch", {
    name: "Enable skill-authoring for Workbench",
  });
  await user.click(activationSwitch);
  const dialog = await screen.findByRole("dialog", { name: "Preview Enable" });

  setViewportWidth(1060);
  expect(document.querySelector(".activation-sheet")).toBe(dialog);
  expect(dialog).toBeInTheDocument();
  expect(background()).toHaveAttribute("inert");

  await user.keyboard("{Escape}");
  expect(
    screen.queryByRole("dialog", { name: "Preview Enable" }),
  ).not.toBeInTheDocument();
  expect(activationSwitch).toHaveFocus();
});

test("busy overlay rejects Escape and backdrop dismissal until the operation settles", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let finishApply: (() => void) | undefined;
  const realApply = client.applyActivation.bind(client);
  client.applyActivation = async (planToken: string) => {
    const result = await realApply(planToken);
    await new Promise<void>((resolve) => {
      finishApply = resolve;
    });
    return result;
  };
  render(<App client={client} />);
  const activationSwitch = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Workbench",
  });
  await user.click(activationSwitch);
  const dialog = await screen.findByRole("dialog", { name: "Preview Enable" });
  await user.click(screen.getByRole("button", { name: "Enable in Workbench" }));

  await user.keyboard("{Escape}");
  expect(dialog).toBeInTheDocument();
  fireEvent.mouseDown(dialog.parentElement as HTMLElement);
  expect(dialog).toBeInTheDocument();

  finishApply?.();
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(activationSwitch).toBeChecked();
});

// -- Notice counts --

test("NoticeRegion: zero collapses, one is single, two stack in the region", async () => {
  const lockClient = createFixtureCatalogClient();
  lockClient.runActivationHealthCheck = async () => {
    throw {
      code: "recovery_required",
      message: "An operation needs recovery.",
    };
  };
  render(<App client={lockClient} />);
  await screen.findByRole("alert");
  expect(noticeRegion().children).toHaveLength(1);
  expect(
    within(noticeRegion()).getByText("Recovery required — writes locked"),
  ).toBeInTheDocument();
});

test("NoticeRegion stacks the Catalog error and the recovery lock together", async () => {
  const stackedClient = createFixtureCatalogClient();
  stackedClient.runActivationHealthCheck = async () => {
    throw {
      code: "recovery_required",
      message: "An operation needs recovery.",
    };
  };
  stackedClient.listSkills = async () => {
    throw new Error("Catalog read failed");
  };
  render(<App client={stackedClient} />);
  await screen.findByRole("heading", { name: "Skill detail unavailable" });
  expect(noticeRegion().children).toHaveLength(2);
  expect(
    within(noticeRegion()).getByText("Library unavailable"),
  ).toBeInTheDocument();
  expect(
    within(noticeRegion()).getByText("Recovery required — writes locked"),
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
    screen.getByText(
      "Import or Adopt a Skill to begin; this is not a loading state.",
    ),
  ).toBeInTheDocument();
  expect(screen.queryByText("Loading Skill detail")).not.toBeInTheDocument();
  expect(screen.getByText("No Skill selected")).toBeInTheDocument();
  expect(
    screen.getByText("Activation controls stay unavailable."),
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
      "The Catalog error is preserved above; this pane is unavailable, not loading.",
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
