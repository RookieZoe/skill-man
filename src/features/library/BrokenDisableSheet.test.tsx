import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type {
  CatalogClient,
  EnablePlan,
  EnableResult,
  GlobalTargetGroupSnapshot,
} from "../../app/catalog-client";
import { BrokenDisableSheet } from "./BrokenDisableSheet";

const mockSnapshot: GlobalTargetGroupSnapshot = {
  skillId: "broken-skill",
  skillName: "Broken Skill",
  agentGeneration: 1,
  groups: [
    {
      targetRootId: "target-root-1",
      configuredPath: "~/.claude/skills",
      desired: true,
      observedState: "dangling",
      availability: "available",
      action: "none",
      diagnostic: null,
      consumers: [
        {
          agentId: "claude",
          agentName: "Claude Code",
          compatibility: "verified",
        },
        {
          agentId: "codex",
          agentName: "Codex",
          compatibility: "verified",
        },
      ],
    },
  ],
};

const mockPlan: EnablePlan = {
  planToken: "plan-token-123",
  scope: "global",
  writeGateGeneration: 1,
  catalogGeneration: 1,
  agentGeneration: 1,
  cells: [],
};

const mockResult: EnableResult = {
  operationId: "op-123",
  snapshotVersion: 1,
  cells: [
    {
      cellKey: "cell-1",
      skillId: "broken-skill",
      targetRootId: "target-root-1",
      outcome: "succeeded",
      diagnostic: null,
    },
  ],
};

function createMockClient(
  overrides: Partial<CatalogClient> = {},
): CatalogClient {
  return {
    listTargetGroups: vi.fn().mockResolvedValue(mockSnapshot),
    planGlobalLifecycle: vi.fn().mockResolvedValue(mockPlan),
    applyGlobalEnable: vi.fn().mockResolvedValue(mockResult),
    ...overrides,
  } as unknown as CatalogClient;
}

describe("BrokenDisableSheet", () => {
  it("renders Step 1 with Broken evidence, target groups, consumers and reappear notice", async () => {
    const client = createMockClient();
    render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        onClose={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("dialog", { name: "Disable Broken Activation" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Broken Activation evidence")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The member path “skills/authoring” vanished from the Source Release, causing global activations to become Broken.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "If the same member (remote_id, skillPath) reappears in a future release, this activation will automatically recover to Healthy.",
      ),
    ).toBeInTheDocument();

    await waitFor(() => {
      expect(
        screen.getByText("Target group: ~/.claude/skills"),
      ).toBeInTheDocument();
      expect(
        screen.getByText("Shared consumers: Claude Code, Codex"),
      ).toBeInTheDocument();
    });

    expect(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    ).toBeInTheDocument();
  });

  it("navigates through Step 1 -> Step 2 -> Apply -> Step 3 (Result) and closes", async () => {
    const client = createMockClient();
    const onClose = vi.fn();
    const user = userEvent.setup();

    render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        onClose={onClose}
      />,
    );

    // Wait for snapshot to load
    await waitFor(() => {
      expect(
        screen.getByText("Target group: ~/.claude/skills"),
      ).toBeInTheDocument();
    });

    // Advance to Step 2
    await user.click(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    );
    expect(
      screen.getByText("Confirm disabling shared target group"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Disabling will remove the broken activation for the entire shared target group and all of its consumers: Claude Code, Codex.",
      ),
    ).toBeInTheDocument();

    // Can go back to Step 1
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByText("Broken Activation evidence")).toBeInTheDocument();

    // Advance to Step 2 again and apply
    await user.click(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    );
    await user.click(
      screen.getByRole("button", { name: "Disable shared target group" }),
    );

    // Verify lifecycle was planned and applied
    expect(client.planGlobalLifecycle).toHaveBeenCalledWith(
      "broken-skill",
      "target-root-1",
      "disable",
    );
    expect(client.applyGlobalEnable).toHaveBeenCalledWith("plan-token-123");

    // Step 3 (Result)
    await waitFor(() => {
      expect(
        screen.getByText("Broken activations disabled"),
      ).toBeInTheDocument();
      expect(
        screen.getByText(
          "The broken activation entry was cleanly removed from the target group.",
        ),
      ).toBeInTheDocument();
    });

    // Close
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("handles Escape and returns focus to opener", async () => {
    const client = createMockClient();
    const onClose = vi.fn();
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    const focusSpy = vi.spyOn(opener, "focus");
    const user = userEvent.setup();

    render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        opener={opener}
        onClose={onClose}
      />,
    );

    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
    expect(focusSpy).toHaveBeenCalled();

    document.body.removeChild(opener);
  });

  it("ignores Escape and backdrop clicks when busy applying", async () => {
    let resolveApply: (val: EnableResult) => void;
    const pendingPromise = new Promise<EnableResult>((res) => {
      resolveApply = res;
    });
    const client = createMockClient({
      applyGlobalEnable: vi.fn().mockReturnValue(pendingPromise),
    });
    const onClose = vi.fn();
    const user = userEvent.setup();

    const { container } = render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        onClose={onClose}
      />,
    );

    await waitFor(() => {
      expect(
        screen.getByText("Target group: ~/.claude/skills"),
      ).toBeInTheDocument();
    });

    await user.click(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    );
    await user.click(
      screen.getByRole("button", { name: "Disable shared target group" }),
    );

    // Press Escape while busy
    await user.keyboard("{Escape}");
    expect(onClose).not.toHaveBeenCalled();

    // Click backdrop while busy
    const backdrop = container.querySelector(".activation-sheet-backdrop");
    expect(backdrop).toBeInTheDocument();
    if (backdrop) {
      await user.click(backdrop);
    }
    expect(onClose).not.toHaveBeenCalled();

    // Cleanup
    resolveApply!(mockResult);
  });

  it("supports low-height viewports with scrollable backdrop and accessible actions", () => {
    const client = createMockClient();
    const { container } = render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        onClose={vi.fn()}
      />,
    );

    const backdrop = container.querySelector(".activation-sheet-backdrop");
    expect(backdrop).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    ).toBeInTheDocument();
  });

  it("preserves state across 1059/1060 breakpoint resize without remounting", async () => {
    const client = createMockClient();
    const user = userEvent.setup();
    render(
      <BrokenDisableSheet
        client={client}
        skillId="broken-skill"
        skillPath="skills/authoring"
        onClose={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(
        screen.getByText("Target group: ~/.claude/skills"),
      ).toBeInTheDocument();
    });
    await user.click(
      screen.getByRole("button", { name: "Continue to confirmation" }),
    );
    expect(
      screen.getByText("Confirm disabling shared target group"),
    ).toBeInTheDocument();

    // Simulate window resize across 1059/1060
    Object.defineProperty(window, "innerWidth", {
      value: 1059,
      configurable: true,
    });
    window.dispatchEvent(new Event("resize"));
    expect(
      screen.getByText("Confirm disabling shared target group"),
    ).toBeInTheDocument();

    Object.defineProperty(window, "innerWidth", {
      value: 1060,
      configurable: true,
    });
    window.dispatchEvent(new Event("resize"));
    expect(
      screen.getByText("Confirm disabling shared target group"),
    ).toBeInTheDocument();
  });
});
