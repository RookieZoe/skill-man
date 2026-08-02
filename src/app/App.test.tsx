import { act, render, screen } from "@testing-library/react";
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

test("shows startup Activation drift and repairs only a missing Activation", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  let repaired = false;
  client.listAgents = async (skillId) =>
    (await listAgents(skillId)).map((agent) => {
      if (agent.id === "claude-code") {
        return {
          ...agent,
          desiredEnabled: true,
          observedState: repaired ? "present" : "missing",
        };
      }
      if (agent.id === "codex") {
        return { ...agent, desiredEnabled: true, observedState: "dangling" };
      }
      return {
        ...agent,
        desiredEnabled: true,
        observedState: "target_mismatch",
      };
    });
  client.planActivationRepair = async () => ({
    planToken: "repair-plan",
    kind: "repair",
    skillDirectoryName: "skill-authoring",
    agentName: "Claude Code",
    enabled: true,
    entryPath: "~/.claude/skills/skill-authoring",
    targetPath: "/Library/skills/skill-authoring",
  });
  client.applyActivation = async () => {
    repaired = true;
    return {
      skillId: "skill-authoring",
      agentId: "claude-code",
      desiredEnabled: true,
      observedState: "present",
      snapshotVersion: 8,
    };
  };
  render(<App client={client} />);

  expect(await screen.findByText("Enabled · Missing")).toBeInTheDocument();
  expect(screen.getByText("Enabled · Dangling")).toBeInTheDocument();
  expect(screen.getByText("Enabled · Target mismatch")).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Repair" })).toHaveLength(1);

  await user.click(screen.getByRole("button", { name: "Repair" }));
  expect(
    await screen.findByRole("dialog", { name: "Preview Repair" }),
  ).toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(screen.getByRole("button", { name: "Repair" })).toHaveFocus();

  await user.click(screen.getByRole("button", { name: "Repair" }));
  await user.click(
    screen.getByRole("button", { name: "Repair in Claude Code" }),
  );

  expect(await screen.findByText("Enabled · Present")).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Repair" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("switch", {
      name: "Enable skill-authoring for Claude Code",
    }),
  ).toHaveFocus();
});

test("refreshes observed state when Repair becomes stale during Apply", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  let dangling = false;
  client.listAgents = async (skillId) =>
    (await listAgents(skillId)).map((agent) =>
      dangling && agent.id === "claude-code"
        ? { ...agent, observedState: "dangling" }
        : agent,
    );
  client.applyActivation = async () => {
    dangling = true;
    throw {
      code: "plan_stale",
      message: "The Skill entity changed after the Repair preview.",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Repair" }));
  await user.click(
    screen.getByRole("button", { name: "Repair in Claude Code" }),
  );

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Skill entity changed after the Repair preview.",
  );
  expect(await screen.findByText("Enabled · Dangling")).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Repair" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("switch", {
      name: "Enable skill-authoring for Claude Code",
    }),
  ).toHaveFocus();
});

test("moves from Repair preview to Conflict when Apply finds occupancy", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  let occupied = false;
  client.listAgents = async (skillId) =>
    (await listAgents(skillId)).map((agent) =>
      occupied && agent.id === "claude-code"
        ? { ...agent, observedState: "occupied" }
        : agent,
    );
  client.applyActivation = async () => {
    occupied = true;
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Repair" }));
  await user.click(
    screen.getByRole("button", { name: "Repair in Claude Code" }),
  );

  expect(
    await screen.findByRole("dialog", { name: "Activation conflict" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Enabled · Occupied")).toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(screen.getByRole("button", { name: "Conflict" })).toHaveFocus();
});

test("opens an explicit Conflict prompt when Repair finds occupied content", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  let occupied = false;
  client.listAgents = async (skillId) =>
    (await listAgents(skillId)).map((agent) =>
      occupied && agent.id === "claude-code"
        ? { ...agent, observedState: "occupied" }
        : agent,
    );
  client.planActivationRepair = async () => {
    occupied = true;
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  render(<App client={client} />);

  const repair = await screen.findByRole("button", { name: "Repair" });
  await user.click(repair);

  expect(
    await screen.findByRole("dialog", { name: "Activation conflict" }),
  ).toHaveTextContent("left it unchanged");
  expect(
    screen.getByText(/conflicts with existing content/),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Keep existing content" }),
  ).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(await screen.findByText("Enabled · Occupied")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Conflict" })).toHaveFocus();
});

test("offers a Conflict entry for an Activation already observed as occupied", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  client.listAgents = async (skillId) =>
    (await listAgents(skillId)).map((agent) =>
      agent.id === "claude-code"
        ? {
            ...agent,
            desiredEnabled: true,
            observedState: "occupied",
          }
        : agent,
    );
  client.planActivationRepair = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  render(<App client={client} />);

  expect(await screen.findByText("Enabled · Occupied")).toBeInTheDocument();
  const conflict = screen.getByRole("button", { name: "Conflict" });
  await user.click(conflict);

  expect(
    await screen.findByRole("dialog", { name: "Activation conflict" }),
  ).toBeInTheDocument();
  await user.keyboard("{Escape}");
  expect(conflict).toHaveFocus();
});

test("keeps persisted Agent state hidden until startup health completes", async () => {
  const client = createFixtureCatalogClient();
  const listAgents = client.listAgents.bind(client);
  let listAgentsCalls = 0;
  client.listAgents = async (skillId) => {
    listAgentsCalls += 1;
    return listAgents(skillId);
  };
  let finishHealthCheck: (() => void) | undefined;
  client.runActivationHealthCheck = () =>
    new Promise((resolve) => {
      finishHealthCheck = () =>
        resolve({
          checked: 1,
          snapshotVersion: 1,
        });
    });
  render(<App client={client} />);

  expect(
    await screen.findByText("Checking desired Activations…"),
  ).toBeVisible();
  expect(screen.queryByText("Enabled · Missing")).not.toBeInTheDocument();
  expect(listAgentsCalls).toBe(0);

  await act(async () => finishHealthCheck?.());

  expect(await screen.findByText("Enabled · Missing")).toBeInTheDocument();
  expect(listAgentsCalls).toBe(1);
});
