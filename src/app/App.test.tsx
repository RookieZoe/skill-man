import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import { App } from "./App";

test("opens the Library Desk with a selected Skill and an Agent inspector", async () => {
  render(<App client={createFixtureCatalogClient()} />);

  expect(
    await screen.findByRole("navigation", { name: "Library" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("main", { name: "Skill detail" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("complementary", { name: "Enable by Agent" }),
  ).toBeInTheDocument();
  expect(
    await screen.findByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();

  const claudeActivation = screen.getByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });
  expect(claudeActivation).toBeChecked();
  expect(claudeActivation).toBeEnabled();
});

test("selects another Skill from the Library without leaving the three-column context", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  const mediaXray = await screen.findByRole("button", { name: "media-xray" });
  await user.click(mediaXray);

  expect(
    await screen.findByRole("heading", { name: "media-xray" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Local changes detected")).toBeInTheDocument();
  expect(
    screen.getByRole("switch", { name: "Enable media-xray for Codex" }),
  ).toBeChecked();
});

test("filters the Library by health and selects the first remaining Skill", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Broken" }));

  expect(
    await screen.findByRole("heading", { name: "legacy-audit" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "media-xray" }),
  ).not.toBeInTheDocument();
  expect(screen.getByText("Source unavailable")).toBeInTheDocument();
});

test("previews and applies Claude Enable from the Agent inspector", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await user.click(await screen.findByRole("button", { name: "media-xray" }));
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable media-xray for Claude Code",
  });
  expect(claudeActivation).not.toBeChecked();

  await user.click(claudeActivation);
  expect(
    await screen.findByRole("dialog", { name: "Preview Enable" }),
  ).toBeInTheDocument();
  expect(screen.getByText("~/.claude/skills/media-xray")).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Enable in Claude Code" }),
  );

  expect(claudeActivation).toBeChecked();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("previews and applies Claude Disable without leaving the detail", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);
  expect(
    await screen.findByRole("dialog", { name: "Preview Disable" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Disable in Claude Code" }),
  ).toHaveFocus();
  await user.tab();
  expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
  await user.tab({ shift: true });
  expect(
    screen.getByRole("button", { name: "Disable in Claude Code" }),
  ).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(claudeActivation).toHaveFocus();

  await user.click(claudeActivation);
  await user.click(
    screen.getByRole("button", { name: "Disable in Claude Code" }),
  );

  expect(claudeActivation).not.toBeChecked();
  expect(
    screen.getByRole("heading", { name: "skill-authoring" }),
  ).toBeInTheDocument();
});

test("keeps the switch unchanged when Activation preflight rejects the path", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.planActivation = async () => {
    throw {
      code: "target_mismatch",
      message: "The Activation no longer matches its recorded target.",
    };
  };
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Activation no longer matches its recorded target.",
  );
  expect(claudeActivation).toBeChecked();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});
