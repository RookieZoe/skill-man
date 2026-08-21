import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";

import { createFixtureCatalogClient } from "../test-fixtures/catalog";
import type {
  AdoptEvidenceCandidate,
  AdoptEvidenceReport,
} from "./catalog-client";
import { App } from "./App";

function evidenceCandidate(
  canonicalEntity: string,
  overrides: Partial<AdoptEvidenceCandidate> = {},
): AdoptEvidenceCandidate {
  const directoryName = canonicalEntity.split("/").pop() ?? canonicalEntity;
  return {
    canonicalEntity,
    directoryName,
    directoryNames: [directoryName],
    appearances: [
      {
        entryPath: canonicalEntity,
        kind: "real_directory",
        agentId: "claude-code",
        shared: false,
        originalTarget: null,
        chain: {
          entryPath: canonicalEntity,
          entryDevice: 1,
          entryInode: 1,
          hops: [],
          finalEntity: canonicalEntity,
          fault: null,
        },
      },
    ],
    verdict: "local",
    reason: { kind: "no_lock" },
    lock: null,
    remote: null,
    localTreeHash: "tree-sha256-v1:abc",
    requiresRelocation: false,
    selectable: true,
    adoptable: true,
    conflict: null,
    suggestedAgentIds: [],
    ...overrides,
  };
}

function evidenceReport(candidates: AdoptEvidenceCandidate[]): {
  generation: number;
  truncated: boolean;
  lockFiles: never[];
  candidates: AdoptEvidenceCandidate[];
} {
  return { generation: 1, truncated: false, lockFiles: [], candidates };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  return {
    promise: new Promise<T>((next) => {
      resolve = next;
    }),
    resolve,
  };
}

function createAdoptableFixtureCatalogClient() {
  const client = createFixtureCatalogClient();
  const canonicalEntity = "~/.claude/skills/prompt-linter";
  client.scanAdopt = async () => ({
    generation: 1,
    truncated: false,
    lockFiles: [],
    candidates: [
      {
        canonicalEntity,
        directoryName: "prompt-linter",
        directoryNames: ["prompt-linter"],
        appearances: [
          {
            entryPath: canonicalEntity,
            kind: "real_directory",
            agentId: "claude-code",
            shared: false,
            originalTarget: null,
            chain: {
              entryPath: canonicalEntity,
              entryDevice: 1,
              entryInode: 1,
              hops: [],
              finalEntity: canonicalEntity,
              fault: null,
            },
          },
        ],
        verdict: "local",
        reason: { kind: "no_lock" },
        lock: null,
        remote: null,
        localTreeHash: "tree-sha256-v1:abc",
        requiresRelocation: false,
        selectable: true,
        adoptable: true,
        conflict: null,
        suggestedAgentIds: [],
      },
    ],
  });
  client.planAdopt = async () => ({
    planToken: "fixture-adopt-plan",
    evidenceGeneration: 1,
    items: [
      {
        directoryName: "prompt-linter",
        canonicalEntity,
        intent: "local_link",
        finalEntityPath: canonicalEntity,
        appearances: [],
        targetAgents: [],
        applyable: true,
        error: null,
      },
    ],
    canApply: true,
  });
  client.applyAdopt = async () => ({
    operationId: "fixture-adopt-operation",
    items: [
      {
        skillId: "prompt-linter",
        directoryName: "prompt-linter",
        adopted: true,
        error: null,
      },
    ],
    snapshotVersion: 8,
    undoAvailable: true,
  });
  client.undoAdopt = async (operationId) => ({
    operationId,
    items: [
      {
        directoryName: "prompt-linter",
        undone: true,
        error: null,
      },
    ],
    snapshotVersion: 9,
  });
  return client;
}

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
    skillId: "skill-authoring",
    agentId: "claude-code",
    kind: "repair",
    skillDirectoryName: "skill-authoring",
    agentName: "Claude Code",
    enabled: true,
    entryPath: "~/.claude/skills/skill-authoring",
    targetPath: "/Library/skills/skill-authoring",
    compatibilityWarning: null,
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
    screen.getByRole("button", { name: "Remove then replace" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Adopt existing item" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
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

test("Cancel from the Conflict sheet leaves everything unchanged", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let plannedReplace = 0;
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  client.planActivationReplace = async () => {
    plannedReplace += 1;
    throw new Error("planActivationReplace must not run on Cancel");
  };
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Cancel" }));

  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(plannedReplace).toBe(0);
  expect(claudeActivation).toHaveFocus();
});

test("disables Adopt existing item when the occupier is not a Skill", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  client.activationConflictDetails = async () => ({
    skillId: "skill-authoring",
    agentId: "claude-code",
    entryPath: "~/.claude/skills/skill-authoring",
    targetPath: "/Library/skills/skill-authoring",
    occupier: {
      kind: "file",
      symlinkTarget: null,
      finalEntityPath: null,
      directoryName: "skill-authoring",
      isSkill: false,
      adoptable: false,
      notAdoptableReason: { kind: "regular_file" },
    },
  });
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);

  const adopt = await screen.findByRole("button", {
    name: "Adopt existing item",
  });
  expect(adopt).toBeDisabled();
  expect(screen.getByText(/not a Skill/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Remove then replace" }),
  ).toBeEnabled();
});

test("Adopt existing item hands off to the Adopt flow with nothing selected", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const scanAdopt = client.scanAdopt;
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  const canonicalEntity = "~/.claude/skills/skill-authoring";
  client.scanAdopt = async () =>
    evidenceReport([evidenceCandidate(canonicalEntity)]);
  void scanAdopt;
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Adopt existing item" }));

  expect(
    await screen.findByRole("dialog", { name: "Adopt untracked Skills" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("dialog", { name: "Activation conflict" }),
  ).not.toBeInTheDocument();
  const checkbox = screen.getByRole("checkbox", {
    name: /Include/,
  });
  expect(checkbox).not.toBeChecked();
  expect(screen.getByRole("button", { name: "Preview Adopt" })).toBeDisabled();
});

test("Adopt conflict handoff leaves an unsafe candidate unselected", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  const canonicalEntity = "~/.claude/skills/skill-authoring";
  client.scanAdopt = async () =>
    evidenceReport([
      evidenceCandidate(canonicalEntity, {
        verdict: "blocked",
        reason: {
          kind: "chain_fault",
          fault: {
            kind: "read_failed",
            at: canonicalEntity,
            detail: "read error",
          },
        },
        selectable: false,
        adoptable: false,
      }),
    ]);
  render(<App client={client} />);

  await user.click(
    await screen.findByRole("switch", {
      name: "Enable skill-authoring for Claude Code",
    }),
  );
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Adopt existing item" }));

  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  expect(
    within(dialog).getByRole("checkbox", { name: "Include" }),
  ).toBeDisabled();
  expect(dialog).not.toHaveTextContent("No selection control");
  expect(
    within(dialog).getByRole("button", { name: "Preview Adopt" }),
  ).toBeDisabled();
});

test("Remove then replace previews, applies, and can be undone", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Remove then replace" }));

  const preview = await screen.findByRole("dialog", {
    name: "Activation conflict",
  });
  expect(preview).toHaveTextContent("Temporary backup");
  expect(preview).toHaveTextContent("Explicit confirmation required");
  await user.click(screen.getByRole("button", { name: "Remove and replace" }));

  const result = await screen.findByRole("dialog", {
    name: "Activation conflict",
  });
  expect(result).toHaveTextContent("Activation created");
  expect(claudeActivation).toBeChecked();
  await user.click(
    screen.getByRole("button", { name: "Restore previous item" }),
  );

  expect(await screen.findByText(/Previous item restored/)).toBeInTheDocument();
  expect(claudeActivation).not.toBeChecked();
  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("finalizes the Replace when Undo was skipped", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let finalized = 0;
  client.planActivation = async () => {
    throw {
      code: "conflict",
      message:
        "The Activation path conflicts with existing content: ~/.claude/skills/skill-authoring",
    };
  };
  client.undoActivationReplace = async () => ({
    undone: false,
    error:
      "the Activation entry was replaced by external content; the original item could not be restored",
    snapshotVersion: 0,
  });
  client.finalizeActivationReplace = async () => {
    finalized += 1;
  };
  render(<App client={client} />);
  const claudeActivation = await screen.findByRole("switch", {
    name: "Enable skill-authoring for Claude Code",
  });

  await user.click(claudeActivation);
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Remove then replace" }));
  await screen.findByRole("dialog", { name: "Activation conflict" });
  await user.click(screen.getByRole("button", { name: "Remove and replace" }));
  await screen.findByText("Activation created");
  await user.click(
    screen.getByRole("button", { name: "Restore previous item" }),
  );
  expect(await screen.findByText(/not restored/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Close" }));

  expect(finalized).toBe(1);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
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

test("imports a linked local folder and continues to its Agent inspector", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  const progress = screen.getByRole("list", { name: "Import progress" });
  expect(within(progress).getByText("Source")).toHaveAttribute(
    "aria-current",
    "step",
  );
  const source = "/Users/zoe/Codes/AI/skills/linked-workflow";
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    source,
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(
    await screen.findByRole("heading", { name: "Preview linked-workflow" }),
  ).toBeInTheDocument();
  expect(within(progress).getByText("Preview")).toHaveAttribute(
    "aria-current",
    "step",
  );
  expect(screen.getAllByText(source)).toHaveLength(2);
  expect(screen.getByText("SQLite pointer only")).toBeInTheDocument();
  expect(screen.getByText("Review imported instructions")).toBeInTheDocument();
  expect(screen.getByText(/can become instructions/)).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Import linked-workflow" }),
  );
  expect(
    await screen.findByRole("dialog", { name: "Link Import result" }),
  ).toHaveTextContent("linked-workflow is Managed");
  expect(within(progress).getByText("Result")).toHaveAttribute(
    "aria-current",
    "step",
  );
  await user.click(screen.getByRole("button", { name: "Enable by Agent" }));

  expect(
    await screen.findByRole("heading", { name: "linked-workflow" }),
  ).toBeInTheDocument();
  const codexActivation = await screen.findByRole("switch", {
    name: "Enable linked-workflow for Codex",
  });
  expect(codexActivation).not.toBeChecked();
  expect(codexActivation).toBeEnabled();
  expect(
    screen.getByRole("switch", {
      name: "Enable linked-workflow for Workbench",
    }),
  ).toBeEnabled();
  await user.click(codexActivation);
  expect(
    await screen.findByRole("dialog", { name: "Preview Enable" }),
  ).toHaveTextContent(source);
});

test("cancels Link discovery without creating a stale Preview", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let finishDiscovery:
    | ((
        candidate: Awaited<ReturnType<typeof client.discoverLinkImport>>,
      ) => void)
    | undefined;
  let planCalls = 0;
  client.discoverLinkImport = () =>
    new Promise((resolve) => {
      finishDiscovery = resolve;
    });
  const planLinkImport = client.planLinkImport.bind(client);
  client.planLinkImport = async (sourcePath) => {
    planCalls += 1;
    return planLinkImport(sourcePath);
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    "/tmp/cancellable-skill",
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(
    within(screen.getByRole("list", { name: "Import progress" })).getByText(
      "Discover",
    ),
  ).toHaveAttribute("aria-current", "step");
  const cancel = screen.getByRole("button", { name: "Cancel" });
  expect(cancel).toBeEnabled();
  await user.click(cancel);
  expect(screen.queryByRole("dialog", { name: "Import Link" })).toBeNull();

  await act(async () => {
    finishDiscovery?.({
      directoryName: "cancellable-skill",
      displayName: "cancellable-skill",
      description: "",
      frontmatterName: null,
      sourceEntryPath: "/tmp/cancellable-skill",
      finalEntityPath: "/tmp/cancellable-skill",
    });
  });
  await waitFor(() => expect(planCalls).toBe(0));
});

test("shows Library Conflict in Link preview and blocks Import", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.type(
    screen.getByRole("textbox", { name: "Local folder path" }),
    "/tmp/skill-authoring",
  );
  await user.click(screen.getByRole("button", { name: "Preview Link" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Library Conflict",
  );
  expect(screen.getByText(/Rename the source/)).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Import skill-authoring" }),
  ).toBeDisabled();
  expect(
    screen.queryByRole("dialog", { name: "Link Import result" }),
  ).not.toBeInTheDocument();
});

test("switches the Import sheet to Git and reports a source rejection", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Import" }));
  await user.click(screen.getByRole("button", { name: "Install from Git" }));
  expect(
    screen.getByRole("dialog", { name: "Import from Git" }),
  ).toBeInTheDocument();
  expect(screen.getByText(/Public HTTPS repository/)).toBeInTheDocument();

  await user.type(
    screen.getByRole("textbox", { name: "Repository URL or owner/repo" }),
    "owner/repo",
  );
  await user.click(screen.getByRole("button", { name: "Discover Skills" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "not available in the preview fixture",
  );
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(
    screen.queryByRole("dialog", { name: "Import from Git" }),
  ).not.toBeInTheDocument();
});

test("checks Skill updates for a remote Install from the detail panel", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "media-xray" }));
  expect(
    await screen.findByRole("heading", { name: "media-xray" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Updates" })).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Check for updates" }));
  expect(await screen.findByRole("status")).toHaveTextContent(
    "This Skill is not tracked for updates.",
  );
});

test("opens the Adopt sheet and reports a fixture rejection", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  expect(
    screen.getByRole("dialog", { name: "Adopt untracked Skills" }),
  ).toBeInTheDocument();
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("Scan failed");
  expect(alert).toHaveTextContent("not available in the preview fixture");
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(
    screen.queryByRole("dialog", { name: "Adopt untracked Skills" }),
  ).not.toBeInTheDocument();
});

test("shows operation content while an Adopt scan is running", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const scanned = deferred<AdoptEvidenceReport>();
  client.scanAdopt = () => scanned.promise;
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));

  const operationWindow = await screen.findByRole("region", {
    name: "Current activity",
  });
  expect(
    within(operationWindow).getByRole("heading", { name: "Scanning" }),
  ).toBeInTheDocument();
  expect(operationWindow).toHaveTextContent(
    "Reading configured Agent and shared Skill directories.",
  );
  expect(
    within(operationWindow).getByRole("progressbar", { name: "Scanning" }),
  ).toBeInTheDocument();

  await act(async () => {
    scanned.resolve(evidenceReport([]));
  });
  await waitFor(() =>
    expect(
      screen.queryByRole("region", { name: "Current activity" }),
    ).not.toBeInTheDocument(),
  );
});

test("labels an Adopt preview failure separately from a scan failure", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.scanAdopt = async () =>
    evidenceReport([evidenceCandidate("~/.claude/skills/prompt-linter")]);
  client.planAdopt = async () => {
    throw {
      code: "validation",
      message: "The selected Skill could not be previewed.",
    };
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  await user.click(within(dialog).getByRole("checkbox", { name: /Include/ }));
  const preview = within(dialog).getByRole("button", {
    name: "Preview Adopt",
  });
  await waitFor(() => expect(preview).toBeEnabled());
  await user.click(preview);

  const alert = await within(dialog).findByRole("alert");
  expect(alert).toHaveTextContent("Preview failed");
  expect(alert).not.toHaveTextContent("Scan failed");
  expect(alert).toHaveTextContent("The selected Skill could not be previewed.");
});

test("leaves an unsafe Adopt candidate unselected and shows its scan reason", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.scanAdopt = async () =>
    evidenceReport([
      evidenceCandidate("~/.agents/skills/ask-matt", {
        verdict: "blocked",
        reason: {
          kind: "chain_fault",
          fault: {
            kind: "read_failed",
            at: "~/.agents/skills/ask-matt",
            detail:
              "Skill symlink target must be a valid UTF-8 relative path: ask-matt",
          },
        },
        selectable: false,
        adoptable: false,
        suggestedAgentIds: ["claude-code", "codex"],
      }),
    ]);
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });

  expect(
    within(dialog).getByRole("checkbox", { name: "Include" }),
  ).toBeDisabled();
  expect(dialog).toHaveTextContent(
    "Skill symlink target must be a valid UTF-8 relative path: ask-matt",
  );
  expect(
    within(dialog).getByRole("button", { name: "Preview Adopt" }),
  ).toBeDisabled();
});

test("keeps a completed Adopt undoable and finalizable when Library refresh fails", async () => {
  const user = userEvent.setup();
  const client = createAdoptableFixtureCatalogClient();
  const listSkills = client.listSkills.bind(client);
  const applyAdopt = client.applyAdopt.bind(client);
  let applied = false;
  let finalizedOperationId: string | null = null;
  client.applyAdopt = async (planToken) => {
    const result = await applyAdopt(planToken);
    applied = true;
    return result;
  };
  client.listSkills = async (filter) => {
    if (applied) {
      throw new Error("Library snapshot unavailable.");
    }
    return listSkills(filter);
  };
  client.finalizeAdopt = async (operationId) => {
    finalizedOperationId = operationId;
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  await user.click(within(dialog).getByRole("checkbox", { name: /Include/ }));
  const previewButton = within(dialog).getByRole("button", {
    name: "Preview Adopt",
  });
  await waitFor(() => expect(previewButton).toBeEnabled());
  await user.click(previewButton);
  await user.click(
    await within(dialog).findByRole("button", { name: /^Adopt$/ }),
  );

  expect(
    await screen.findByRole("heading", { name: "1 of 1 Skills adopted" }),
  ).toBeInTheDocument();
  const alert = screen.getByRole("alert");
  expect(alert).toHaveTextContent("Refresh failed");
  expect(alert).toHaveTextContent(
    "Adopt completed, but the Library refresh failed: Library snapshot unavailable.",
  );
  expect(
    screen.getByRole("button", { name: "Undo this batch" }),
  ).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close" }));
  await waitFor(() =>
    expect(finalizedOperationId).toBe("fixture-adopt-operation"),
  );
});

test("keeps a completed Undo when Library refresh fails", async () => {
  const user = userEvent.setup();
  const client = createAdoptableFixtureCatalogClient();
  const listSkills = client.listSkills.bind(client);
  const undoAdopt = client.undoAdopt.bind(client);
  let undone = false;
  client.undoAdopt = async (operationId) => {
    const result = await undoAdopt(operationId);
    undone = true;
    return result;
  };
  client.listSkills = async (filter) => {
    if (undone) {
      throw new Error("Library snapshot unavailable after Undo.");
    }
    return listSkills(filter);
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  await user.click(within(dialog).getByRole("checkbox", { name: /Include/ }));
  const previewButton = within(dialog).getByRole("button", {
    name: "Preview Adopt",
  });
  await waitFor(() => expect(previewButton).toBeEnabled());
  await user.click(previewButton);
  await user.click(
    await within(dialog).findByRole("button", { name: /^Adopt$/ }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Undo this batch" }),
  );

  expect(await within(dialog).findByRole("status")).toHaveTextContent(
    "Batch undone",
  );
  const alert = within(dialog).getByRole("alert");
  expect(alert).toHaveTextContent("Refresh failed");
  expect(alert).toHaveTextContent(
    "Undo completed, but the Library refresh failed: Library snapshot unavailable after Undo.",
  );
  expect(
    within(dialog).queryByRole("button", { name: "Undo this batch" }),
  ).not.toBeInTheDocument();
});

test("reports an Adopt command failure before showing a result", async () => {
  const user = userEvent.setup();
  const client = createAdoptableFixtureCatalogClient();
  client.applyAdopt = async () => {
    throw new Error("Adopt command rejected the plan.");
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  await user.click(within(dialog).getByRole("checkbox", { name: /Include/ }));
  const previewButton = within(dialog).getByRole("button", {
    name: "Preview Adopt",
  });
  await waitFor(() => expect(previewButton).toBeEnabled());
  await user.click(previewButton);
  await user.click(
    await within(dialog).findByRole("button", { name: /^Adopt$/ }),
  );

  const alert = await within(dialog).findByRole("alert");
  expect(alert).toHaveTextContent("Adopt failed");
  expect(alert).toHaveTextContent("Adopt command rejected the plan.");
  expect(
    within(dialog).queryByRole("heading", { name: /Skills adopted/ }),
  ).not.toBeInTheDocument();
});

test("reports an Undo command failure and keeps Undo available", async () => {
  const user = userEvent.setup();
  const client = createAdoptableFixtureCatalogClient();
  client.undoAdopt = async () => {
    throw new Error("Undo command rejected the operation.");
  };
  render(<App client={client} />);
  await screen.findByRole("heading", { name: "skill-authoring" });

  await user.click(screen.getByRole("button", { name: "Adopt" }));
  const dialog = await screen.findByRole("dialog", {
    name: "Adopt untracked Skills",
  });
  await user.click(within(dialog).getByRole("checkbox", { name: /Include/ }));
  const previewButton = within(dialog).getByRole("button", {
    name: "Preview Adopt",
  });
  await waitFor(() => expect(previewButton).toBeEnabled());
  await user.click(previewButton);
  await user.click(
    await within(dialog).findByRole("button", { name: /^Adopt$/ }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Undo this batch" }),
  );

  const alert = await within(dialog).findByRole("alert");
  expect(alert).toHaveTextContent("Undo failed");
  expect(alert).toHaveTextContent("Undo command rejected the operation.");
  expect(
    within(dialog).getByRole("button", { name: "Undo this batch" }),
  ).toBeInTheDocument();
});

test("shows the three-step onboarding on first run and Skip records completion", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  let completed = 0;
  client.startupInfo = async () => ({
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [
      {
        id: "claude-code",
        name: "Claude Code",
        kind: "claude_preset",
        skillsPath: "~/.claude/skills",
        detected: true,
      },
      {
        id: "codex",
        name: "Codex",
        kind: "codex_preset",
        skillsPath: "~/.codex/skills",
        detected: false,
      },
    ],
  });
  client.completeOnboarding = async () => {
    completed += 1;
  };
  render(<App client={client} />);

  const dialog = await screen.findByRole("dialog", {
    name: "Welcome to Skill Man",
  });
  expect(dialog).toHaveTextContent("Your Library");
  expect(dialog).toHaveTextContent("/Users/zoe/SkillMan");
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(await screen.findByText("Check Agent Presets")).toBeInTheDocument();
  expect(screen.getByText("Not detected")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Skip setup" }));

  expect(completed).toBe(1);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(
    await screen.findByRole("main", { name: "Skill detail" }),
  ).toBeInTheDocument();
});

test("returns to the Library step when preset checking fails", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const info = {
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [],
  };
  let startupCalls = 0;
  client.startupInfo = () => {
    startupCalls += 1;
    return startupCalls === 1
      ? Promise.resolve(info)
      : Promise.reject(new Error("preset check failed"));
  };
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "Your Library" });
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "preset check failed",
  );
  expect(
    screen.getByRole("heading", { name: "Your Library" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("heading", { name: "Check Agent Presets" }),
  ).not.toBeInTheDocument();
});

test("shows honest busy progress while checking presets and scanning untracked Skills", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  const info = {
    firstRun: true,
    libraryPath: "/Users/zoe/SkillMan",
    agents: [],
  };
  const checked = deferred<typeof info>();
  const scanned = deferred<ReturnType<typeof evidenceReport>>();
  let startupCalls = 0;
  client.startupInfo = () => {
    startupCalls += 1;
    return startupCalls === 1 ? Promise.resolve(info) : checked.promise;
  };
  client.scanAdopt = () => scanned.promise;
  render(<App client={client} />);

  await screen.findByRole("heading", { name: "Your Library" });
  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(
    await screen.findByRole("progressbar", {
      name: "Checking Agent presets…",
    }),
  ).toHaveAttribute("aria-valuetext", "Checking Agent presets…");

  checked.resolve(info);
  await screen.findByRole("heading", { name: "Check Agent Presets" });
  await waitFor(() =>
    expect(
      screen.queryByRole("progressbar", {
        name: "Checking Agent presets…",
      }),
    ).not.toBeInTheDocument(),
  );

  await user.click(screen.getByRole("button", { name: "Continue" }));
  expect(
    await screen.findByRole("progressbar", {
      name: "Scanning Agent and shared directories…",
    }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("heading", { name: "Scan existing Skills" }),
  ).toBeInTheDocument();

  scanned.resolve(
    evidenceReport([
      evidenceCandidate("~/.claude/skills/prompt-linter", {
        verdict: "verified",
      }),
    ]),
  );
  await screen.findByText("1 Untracked Skill found. Nothing changed yet.");
  const untrackedList = document.querySelector(".onboarding-untracked-list");
  expect(untrackedList).toBeInTheDocument();
  expect(untrackedList).toHaveTextContent("~/.claude/skills/prompt-linter");
  expect(untrackedList).toHaveTextContent("Verified Remote Source");
});

test("onboarding full scan hands off to Adopt with nothing selected", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.startupInfo = async () => ({
    firstRun: true,
    agents: [],
  });
  client.scanAdopt = async () =>
    evidenceReport([
      evidenceCandidate("~/.claude/skills/prompt-linter"),
      evidenceCandidate("~/.agents/skills/ask-matt", {
        verdict: "blocked",
        reason: {
          kind: "chain_fault",
          fault: {
            kind: "read_failed",
            at: "~/.agents/skills/ask-matt",
            detail:
              "Skill symlink target must be a valid UTF-8 relative path: ask-matt",
          },
        },
        selectable: false,
        adoptable: false,
        suggestedAgentIds: ["claude-code", "codex"],
      }),
    ]);
  render(<App client={client} />);

  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(
    await screen.findByText(/2 Untracked Skills found/),
  ).toBeInTheDocument();
  expect(screen.getByText("Blocked")).toBeInTheDocument();
  await user.click(
    screen.getByRole("button", { name: "Review Adopt candidates" }),
  );

  expect(
    await screen.findByRole("dialog", { name: "Adopt untracked Skills" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("dialog", { name: "Welcome to Skill Man" }),
  ).not.toBeInTheDocument();
  // Viewing is never selecting: the handoff opens the ledger with every
  // Include control unchecked, including disabled controls for blocked rows.
  const includeControls = screen.getAllByRole("checkbox", { name: /Include/ });
  expect(includeControls).toHaveLength(2);
  for (const includeControl of includeControls) {
    expect(includeControl).not.toBeChecked();
  }
  expect(
    includeControls.filter((control) => (control as HTMLInputElement).disabled),
  ).toHaveLength(1);
  expect(
    screen.queryByRole("checkbox", { name: /ask-matt/ }),
  ).not.toBeInTheDocument();
});

test("Preferences sheet shows exactly four switches with defaults", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));

  const dialog = await screen.findByRole("dialog", { name: "Preferences" });
  expect(dialog).toHaveTextContent("Exactly four switches");
  const launch = screen.getByRole("switch", { name: "Launch at login" });
  expect(launch).not.toBeChecked();
  expect(screen.getByRole("switch", { name: "Show in Dock" })).toBeChecked();
  expect(
    screen.getByRole("switch", { name: "Check for app updates" }),
  ).toBeChecked();
  expect(
    screen.getByRole("switch", { name: "Check for Skill updates" }),
  ).toBeChecked();
  expect(dialog.querySelectorAll('input[type="checkbox"]')).toHaveLength(4);

  await user.click(launch);
  expect(launch).toBeChecked();
  await user.click(screen.getByRole("button", { name: "Done" }));
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

test("downloads an available app update before asking to install and restart", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Adds signed, verified app updates.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-update-1",
  });
  let finishDownload:
    ((downloaded: { updateId: string; version: string }) => void) | undefined;
  client.downloadAppUpdate = () =>
    new Promise<{ updateId: string; version: string }>((resolve) => {
      finishDownload = resolve;
    });
  client.installAppUpdate = async (updateId) => {
    if (updateId !== "fixture-update-1") {
      throw new Error("The downloaded update was not installed.");
    }
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));

  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  expect(dialog).toHaveTextContent("0.2.0");
  expect(dialog).toHaveTextContent("Current version 0.1.0");
  expect(dialog).toHaveTextContent("Adds signed, verified app updates.");
  expect(dialog).toHaveTextContent("12 MB");

  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  expect(
    within(dialog).queryByRole("button", { name: "Install and Restart" }),
  ).not.toBeInTheDocument();

  await act(async () =>
    finishDownload?.({ updateId: "fixture-update-1", version: "0.2.0" }),
  );
  await user.click(
    await within(dialog).findByRole("button", {
      name: "Install and Restart",
    }),
  );
  await waitFor(() => {
    expect(
      screen.queryByRole("dialog", { name: "App update available" }),
    ).not.toBeInTheDocument();
  });
});

test("cancels an in-flight app update download without making it installable", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "A cancellable update.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-cancel-download",
  });
  let rejectDownload: ((reason: unknown) => void) | undefined;
  client.downloadAppUpdate = () =>
    new Promise((_, reject) => {
      rejectDownload = reject;
    });
  const cancelled: string[] = [];
  client.cancelAppUpdate = async (updateId) => {
    cancelled.push(updateId);
    rejectDownload?.({ code: "update_cancelled", message: "cancelled" });
    return { updateId };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  const cancelDownload = within(dialog).getByRole("button", {
    name: "Cancel download",
  });
  expect(cancelDownload).toHaveFocus();
  await user.click(cancelDownload);

  expect(cancelled).toEqual(["fixture-cancel-download"]);
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Install and Restart" }),
  ).not.toBeInTheDocument();
});

test("discards a downloaded app update when Later is chosen", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Discard after verification.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-discard-download",
  });
  client.downloadAppUpdate = async (updateId) => ({
    updateId,
    version: "0.2.0",
  });
  const cancelled: string[] = [];
  client.cancelAppUpdate = async (updateId) => {
    cancelled.push(updateId);
    return { updateId };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Later" }),
  );

  expect(cancelled).toEqual(["fixture-discard-download"]);
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
});

test("keeps a downloaded app update visible when discard fails", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "Keep verified bytes until discard succeeds.",
    downloadSizeBytes: 12 * 1024 * 1024,
    updateId: "fixture-discard-failure",
  });
  client.downloadAppUpdate = async (updateId) => ({
    updateId,
    version: "0.2.0",
  });
  client.cancelAppUpdate = async () => {
    throw {
      code: "state_unavailable",
      message: "Could not release the downloaded update.",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));
  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  await user.click(
    within(dialog).getByRole("button", { name: "Download update" }),
  );
  await user.click(
    await within(dialog).findByRole("button", { name: "Later" }),
  );

  expect(dialog).toBeInTheDocument();
  expect(await within(dialog).findByRole("alert")).toHaveTextContent(
    "The update state could not be read. Restart Skill Man and retry.",
  );
  expect(
    within(dialog).getByRole("button", { name: "Install and Restart" }),
  ).toBeEnabled();
});

test("checks for an app update at startup when the preference is enabled", async () => {
  const client = createFixtureCatalogClient();
  client.checkAppUpdate = async (force) => {
    if (force) throw new Error("Expected the scheduled update check.");
    return {
      status: "available",
      version: "0.2.0",
      currentVersion: "0.1.0",
      releaseNotes: "A scheduled update is ready.",
      downloadSizeBytes: 4 * 1024 * 1024,
      updateId: "fixture-scheduled-update",
    };
  };

  render(<App client={client} />);

  expect(
    await screen.findByRole("dialog", { name: "App update available" }),
  ).toHaveTextContent("A scheduled update is ready.");
});

test("keeps a failed offline startup update check silent", async () => {
  const client = createFixtureCatalogClient();
  let attempted = false;
  client.checkAppUpdate = async () => {
    attempted = true;
    throw new Error("The update server is offline.");
  };

  render(<App client={client} />);

  await waitFor(() => expect(attempted).toBe(true));
  expect(
    screen.queryByText(/update server is offline/i),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
});

test("shows manual app update failures in Preferences", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => {
    throw {
      code: "source_unavailable",
      message: "The update server is offline.",
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await user.click(screen.getByRole("button", { name: "Check now" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Unable to reach the update service. Check your network and retry.",
  );
  expect(
    screen.getByRole("dialog", { name: "Preferences" }),
  ).toBeInTheDocument();
});

test("closes the app update sheet with Escape and restores background focus", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.loadPreferences = async () => ({
    launchAtLogin: false,
    showInDock: true,
    checkAppUpdates: false,
    checkSkillUpdates: true,
  });
  client.checkAppUpdate = async () => ({
    status: "available",
    version: "0.2.0",
    currentVersion: "0.1.0",
    releaseNotes: "A focus-safe update.",
    downloadSizeBytes: 1024,
    updateId: "fixture-focus-update",
  });
  const { container } = render(<App client={client} />);

  const preferencesTrigger = await screen.findByRole("button", {
    name: "Preferences",
  });
  await user.click(preferencesTrigger);
  await user.click(screen.getByRole("button", { name: "Check now" }));

  const dialog = await screen.findByRole("dialog", {
    name: "App update available",
  });
  const downloadButton = within(dialog).getByRole("button", {
    name: "Download update",
  });
  expect(downloadButton).toHaveFocus();
  await user.tab();
  expect(within(dialog).getByRole("button", { name: "Not now" })).toHaveFocus();
  await user.tab();
  expect(downloadButton).toHaveFocus();
  expect(container.querySelector(".app-background")).toHaveAttribute("inert");

  await user.keyboard("{Escape}");

  expect(
    screen.queryByRole("dialog", { name: "App update available" }),
  ).not.toBeInTheDocument();
  expect(preferencesTrigger).toHaveFocus();
  expect(container.querySelector(".app-background")).not.toHaveAttribute(
    "inert",
  );
});

test("Preferences warning from the backend is shown inline", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.updatePreferences = async (updates) => ({
    preferences: {
      launchAtLogin: updates.launchAtLogin ?? false,
      showInDock: true,
      checkAppUpdates: true,
      checkSkillUpdates: true,
    },
    warning: {
      kind: "launch_at_login_failed",
      detail: "login item unavailable in dev build",
    },
  });
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "Preferences" }));
  await screen.findByRole("dialog", { name: "Preferences" });
  await user.click(screen.getByRole("switch", { name: "Launch at login" }));

  expect(await screen.findByText(/login item unavailable/)).toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Launch at login" })).toBeChecked();
});

test("onboarding creates a missing Agent directory with explicit confirmation", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.startupInfo = async () => ({
    firstRun: true,
    agents: [
      {
        id: "custom-workbench",
        name: "Custom Workbench",
        kind: "custom",
        skillsPath: "~/.custom-tools/skills",
        detected: false,
      },
    ],
  });
  client.createAgentDirectory = async () => ({
    firstRun: true,
    agents: [
      {
        id: "custom-workbench",
        name: "Custom Workbench",
        kind: "custom",
        skillsPath: "~/.custom-tools/skills",
        detected: true,
      },
    ],
  });
  render(<App client={client} />);

  await screen.findByRole("dialog", { name: "Welcome to Skill Man" });
  await user.click(screen.getByRole("button", { name: "Continue" }));

  expect(await screen.findByText("Not detected")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Create directory" }));

  expect(await screen.findByText("Detected")).toBeInTheDocument();
  await waitFor(() => {
    expect(
      screen.queryByRole("button", { name: "Create directory" }),
    ).not.toBeInTheDocument();
  });
});

test("Broken Link detail offers Relocate and restores health after preview confirm", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await user.click(screen.getByRole("button", { name: "Broken" }));
  await screen.findByRole("heading", { name: "legacy-audit" });
  expect(screen.getByText("Source unavailable")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Relocate…" }));
  expect(
    screen.getByRole("dialog", { name: "Relocate Broken Link" }),
  ).toBeInTheDocument();

  await user.type(
    screen.getByLabelText("New source path"),
    "~/Projects/moved/legacy-audit",
  );
  await user.click(screen.getByRole("button", { name: "Preview Relocate" }));

  expect(
    await screen.findByRole("heading", { name: "Relocate legacy-audit" }),
  ).toBeInTheDocument();
  expect(
    screen.getAllByText("~/Projects/moved/legacy-audit").length,
  ).toBeGreaterThan(0);
  expect(screen.getByText("Activations to update")).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Relocate" }));
  expect(
    await screen.findByRole("heading", {
      name: "legacy-audit is healthy again",
    }),
  ).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.getByText("Healthy")).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Relocate…" }),
  ).not.toBeInTheDocument();
});

test("Modified Install update offers only abandon-and-update or cancel", async () => {
  const user = userEvent.setup();
  const client = createFixtureCatalogClient();
  client.checkSkillUpdates = async () => ({
    groups: [
      {
        repoUrl: "RookieZoe/media-xray",
        items: [
          {
            skillId: "media-xray",
            directoryName: "media-xray",
            sourceUrl: "RookieZoe/media-xray",
            requestedRef: "HEAD",
            currentCommit: "1111111111",
            resolvedCommit: "2222222222",
            hasUpdate: true,
            modified: true,
            upstreamPathGone: false,
            lastCheckedAt: null,
          },
        ],
      },
    ],
    errors: [],
    parentConflicts: [],
  });
  client.planSkillUpdates = async () => ({
    items: [
      {
        skillId: "media-xray",
        directoryName: "media-xray",
        planToken: "fixture-update-plan-1",
        currentCommit: "1111111111",
        newCommit: "2222222222",
        modified: true,
        pathChanged: false,
        error: null,
      },
    ],
  });
  let abandoned = false;
  client.applySkillUpdates = async (requests, abandonChanges) => {
    abandoned = abandonChanges;
    return {
      items: [
        {
          skillId: "media-xray",
          directoryName: "media-xray",
          updated: true,
          error: null,
        },
      ],
    };
  };
  render(<App client={client} />);

  await user.click(await screen.findByRole("button", { name: "media-xray" }));
  await user.click(screen.getByRole("button", { name: "Check for updates" }));

  expect(await screen.findByRole("status")).toHaveTextContent(
    "Update available",
  );
  expect(
    screen.queryByRole("button", { name: "Keep current version" }),
  ).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Update" }));
  expect(
    await screen.findByRole("button", { name: "Abandon changes and update" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Keep current version" }),
  ).not.toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Abandon changes and update" }),
  );
  expect(await screen.findByText("Update applied.")).toBeInTheDocument();
  expect(abandoned).toBe(true);
});

test("removes a Managed Skill after preview confirmation", async () => {
  const user = userEvent.setup();
  render(<App client={createFixtureCatalogClient()} />);

  await screen.findByRole("heading", { name: "skill-authoring" });
  await user.click(screen.getByRole("button", { name: "Remove…" }));

  expect(
    await screen.findByRole("dialog", { name: "Remove Managed Skill" }),
  ).toBeInTheDocument();
  expect(
    await screen.findByText("The external Link entity is kept in place"),
  ).toBeInTheDocument();
  expect(screen.getByText("Activations to disable")).toBeInTheDocument();

  await user.click(
    screen.getByRole("button", { name: "Remove skill-authoring" }),
  );
  expect(
    await screen.findByRole("heading", {
      name: "skill-authoring left the Library",
    }),
  ).toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(
    screen.queryByRole("heading", { name: "skill-authoring" }),
  ).not.toBeInTheDocument();
});

test("shows the recovery-required lock notice with retry", async () => {
  const client = createFixtureCatalogClient();
  let attempts = 0;
  client.runActivationHealthCheck = async () => {
    attempts += 1;
    if (attempts === 1) {
      throw {
        code: "recovery_required",
        message: "interrupted Remove could not be recovered",
      };
    }
    return { checked: 1, snapshotVersion: 1 };
  };
  const user = userEvent.setup();
  render(<App client={client} />);

  const notice = await screen.findByRole("alert", { hidden: false });
  expect(notice).toHaveTextContent("Recovery required — writes locked");
  expect(notice).toHaveTextContent("interrupted Remove could not be recovered");

  await user.click(screen.getByRole("button", { name: "Retry recovery" }));
  await waitFor(() => {
    expect(
      screen.queryByText("Recovery required — writes locked"),
    ).not.toBeInTheDocument();
  });
});
