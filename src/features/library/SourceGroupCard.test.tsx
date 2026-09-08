import { BackgroundOperations } from "../../ui/BackgroundOperations";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type {
  GitSourceCapabilitySource,
  SkillSummary,
} from "../../app/catalog-client";
import { SourceGroupCard } from "./SourceGroupCard";

const healthySource: GitSourceCapabilitySource = {
  remoteId: "source-1",
  canonicalUrl: "https://github.com/org/repo",
  kind: "git_repository_source",
  provider: "github",
  trackingMode: "auto_release_tag_head",
  trackingValue: null,
  selectedRef: "v1.0.0",
  resolvedCommit: "abc1234567890",
  members: [
    {
      skillId: "skill-1",
      skillPath: "skills/alpha",
      presence: true,
    },
    {
      skillId: "skill-2",
      skillPath: "skills/beta",
      presence: true,
    },
  ],
};

const skills: SkillSummary[] = [
  {
    id: "skill-1",
    directoryName: "alpha",
    displayName: "Alpha Skill",
    description: "First skill",
    sourceKind: "remote_install",
    health: "healthy",
    enabledAgentCount: 2,
  },
  {
    id: "skill-2",
    directoryName: "beta",
    displayName: "Beta Skill",
    description: "Second skill",
    sourceKind: "remote_install",
    health: "healthy",
    enabledAgentCount: 0,
  },
];

describe("SourceGroupCard", () => {
  it("shows and copies a repository root member using its skill directory name", async () => {
    const user = userEvent.setup();
    const onCopyMember = vi.fn().mockResolvedValue(true);
    render(
      <BackgroundOperations>
        <SourceGroupCard
          source={{
            ...healthySource,
            members: [{ ...healthySource.members[0], skillPath: "" }],
          }}
          skills={skills}
          onCopyMember={onCopyMember}
          pickDirectory={async () => "/Users/test/copies"}
        />
      </BackgroundOperations>,
    );
    expect(screen.getByText("repo root")).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "repo root" }));
    await user.click(screen.getByRole("button", { name: "Create local copy" }));
    await waitFor(() =>
      expect(onCopyMember).toHaveBeenCalledExactlyOnceWith(
        "source-1",
        "skill-1",
        "/Users/test/copies/alpha",
      ),
    );
  });

  it("uses a dismissible confirmation popover and only removes after explicit confirmation", async () => {
    const user = userEvent.setup();
    const remove = vi.fn();
    render(
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onRemove={remove}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Remove" });
    await user.click(trigger);
    expect(screen.getByRole("dialog")).toHaveTextContent(
      healthySource.canonicalUrl,
    );
    expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    await user.click(trigger);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await user.click(trigger);
    await user.click(document.body);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(remove).not.toHaveBeenCalled();
    await user.click(trigger);
    await user.click(
      screen.getByRole("button", { name: "Remove complete source" }),
    );
    expect(remove).toHaveBeenCalledExactlyOnceWith("source-1");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
  it("copies selected members sequentially and preserves failed selections", async () => {
    const user = userEvent.setup();
    const onCopyMember = vi
      .fn()
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(false);
    const picker = vi.fn().mockResolvedValue("/Users/test/copies");
    render(
      <BackgroundOperations>
        <SourceGroupCard
          source={healthySource}
          skills={skills}
          onCopyMember={onCopyMember}
          pickDirectory={picker}
        />
      </BackgroundOperations>,
    );
    await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
    await user.click(screen.getByRole("checkbox", { name: "skills/beta" }));
    await user.click(screen.getByRole("button", { name: "Create local copy" }));
    expect(
      await screen.findByRole("region", { name: "Current activity" }),
    ).toHaveTextContent("Copy stopped: 1 of 2 completed.");
    expect(picker).toHaveBeenCalledTimes(1);
    expect(onCopyMember.mock.calls).toEqual([
      ["source-1", "skill-1", "/Users/test/copies/alpha"],
      ["source-1", "skill-2", "/Users/test/copies/beta"],
    ]);
    expect(
      screen.getByRole("checkbox", { name: "skills/alpha" }),
    ).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "skills/beta" })).toBeChecked();
  });

  it("cancelling the directory picker does not create copies or clear selection", async () => {
    const user = userEvent.setup();
    const copy = vi.fn();
    render(
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onCopyMember={copy}
        pickDirectory={async () => null}
      />,
    );
    await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
    await user.click(screen.getByRole("button", { name: "Create local copy" }));
    expect(copy).not.toHaveBeenCalled();
    expect(
      screen.getByRole("checkbox", { name: "skills/alpha" }),
    ).toBeChecked();
  });
  it("renders policy cascade side-by-side with explicit override", () => {
    render(<SourceGroupCard source={healthySource} skills={skills} />);

    expect(screen.getByText("Source Tracking Policy")).toBeInTheDocument();
    expect(
      screen.getByText("Latest official provider release"),
    ).toBeInTheDocument();
    expect(screen.getByText("Highest stable SemVer tag")).toBeInTheDocument();
    expect(
      screen.getByText("Latest tag reachable from default branch"),
    ).toBeInTheDocument();
    expect(screen.getByText("HEAD")).toBeInTheDocument();
    expect(screen.getByText("Version override")).toBeInTheDocument();
    expect(screen.getByText("None")).toBeInTheDocument();
  });

  it("renders member rows read-only with skillPath, health, and target group count without version actions", () => {
    render(
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onUpdate={vi.fn()}
      />,
    );

    expect(screen.getByText("skills/alpha")).toBeInTheDocument();
    expect(screen.getByText("skills/beta")).toBeInTheDocument();
    expect(screen.getByText("2 target groups")).toBeInTheDocument();
    expect(screen.getByText("0 target groups")).toBeInTheDocument();

    // No member-level version actions (no Update button per member)
    const updateButtons = screen.getAllByRole("button", { name: "Update" });
    expect(updateButtons).toHaveLength(1); // Only the source-level update button
    expect(screen.getAllByText("Healthy")).toHaveLength(2);
  });

  it("offers a Local Copy exit for healthy Git members", async () => {
    const user = userEvent.setup();
    const onCopyMember = vi.fn();
    render(
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onCopyMember={onCopyMember}
        pickDirectory={async () => "/Users/test/Desktop/copied"}
      />,
    );

    const copy = screen.getByRole("button", { name: "Create local copy" });
    expect(copy).toBeDisabled();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
    await user.click(copy);
    expect(onCopyMember).toHaveBeenCalledWith(
      "source-1",
      "skill-1",
      "/Users/test/Desktop/copied/alpha",
    );
  });

  it("renders embedded Source Snapshot Mismatch panel and disables Update when a member has snapshot mismatch", async () => {
    const mismatchedSkills: SkillSummary[] = [
      {
        ...skills[0],
        health: "source_snapshot_mismatch",
      },
      skills[1],
    ];
    const onRestore = vi.fn();
    const onUpdate = vi.fn();
    const onCopyMember = vi.fn();
    const user = userEvent.setup();

    render(
      <SourceGroupCard
        source={healthySource}
        skills={mismatchedSkills}
        onUpdate={onUpdate}
        onRestore={onRestore}
        onCopyMember={onCopyMember}
        pickDirectory={async () => "/Users/test/Desktop/copied"}
      />,
    );

    // Mismatch panel declares Update & new Enable are blocked
    expect(screen.getByText("Source Snapshot Mismatch")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Update and new Enable are blocked. Member snapshots no longer match the current Source Release.",
      ),
    ).toBeInTheDocument();

    // Update button is disabled
    const updateButton = screen.getByRole("button", { name: "Update" });
    expect(updateButton).toBeDisabled();

    // Restore Current Source Release with confirmation
    const restoreBtn = screen.getByRole("button", {
      name: "Restore current Source Release",
    });
    await user.click(restoreBtn);
    const confirmRestoreBtn = screen.getByRole("button", {
      name: "Restore release",
    });
    await user.click(confirmRestoreBtn);
    expect(onRestore).toHaveBeenCalledWith("source-1");

    // Create Local Source Copy
    const copyBtn = screen.getByRole("button", {
      name: "Create local copy",
    });
    await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
    await user.click(copyBtn);
    expect(onCopyMember).toHaveBeenCalledWith(
      "source-1",
      expect.any(String),
      "/Users/test/Desktop/copied/alpha",
    );
  });

  it("provides entry to dedicated Broken disable sheet when member vanished/tombstoned", async () => {
    const sourceWithBrokenMember: GitSourceCapabilitySource = {
      ...healthySource,
      members: [
        {
          skillId: "skill-1",
          skillPath: "skills/alpha",
          presence: false, // tombstoned/vanished
        },
      ],
    };
    const brokenSkills: SkillSummary[] = [
      {
        ...skills[0],
        health: "broken",
      },
    ];
    const onOpenBrokenDisable = vi.fn();
    const user = userEvent.setup();

    render(
      <SourceGroupCard
        source={sourceWithBrokenMember}
        skills={brokenSkills}
        onOpenBrokenDisable={onOpenBrokenDisable}
      />,
    );

    expect(screen.getByText("removed upstream")).toBeInTheDocument();
    const disableBtn = screen.getByRole("button", {
      name: "Disable broken activations…",
    });
    await user.click(disableBtn);
    expect(onOpenBrokenDisable).toHaveBeenCalledWith(
      "skill-1",
      "skills/alpha",
      expect.any(HTMLButtonElement),
    );
  });

  it("fails closed for legacy and identity conflict sources with typed reason", () => {
    const legacySource: GitSourceCapabilitySource = {
      remoteId: "legacy-1",
      canonicalUrl: "https://github.com/org/legacy",
      kind: "legacy_per_skill_git_state",
      members: [],
    };
    const conflictSource: GitSourceCapabilitySource = {
      remoteId: "conflict-1",
      canonicalUrl: "https://github.com/org/conflict",
      kind: "remote_source_identity_conflict",
      members: [],
    };

    const { rerender } = render(
      <SourceGroupCard source={legacySource} skills={[]} />,
    );
    expect(
      screen.getByText(
        "Upgrade the legacy source before managing this repository.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Update" }),
    ).not.toBeInTheDocument();

    rerender(<SourceGroupCard source={conflictSource} skills={[]} />);
    expect(
      screen.getByText(
        "Repository actions are unavailable because source records conflict.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Update" }),
    ).not.toBeInTheDocument();
  });
});

it("continues a confirmed copy batch after its card unmounts", async () => {
  const user = userEvent.setup();
  let resolve!: (value: boolean) => void;
  const pending = new Promise<boolean>((done) => {
    resolve = done;
  });
  const copy = vi.fn().mockReturnValueOnce(pending).mockResolvedValue(true);
  const view = render(
    <BackgroundOperations>
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onCopyMember={copy}
        pickDirectory={async () => "/tmp/copies"}
      />
    </BackgroundOperations>,
  );
  await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
  await user.click(screen.getByRole("checkbox", { name: "skills/beta" }));
  await user.click(screen.getByRole("button", { name: "Create local copy" }));
  expect(copy).toHaveBeenCalledTimes(1);
  view.rerender(
    <BackgroundOperations>
      <p>Another page</p>
    </BackgroundOperations>,
  );
  await act(async () => {
    resolve(true);
    await pending;
  });
  await waitFor(() => expect(copy).toHaveBeenCalledTimes(2));
  expect(
    await screen.findByRole("region", { name: "Current activity" }),
  ).toHaveTextContent("Created 2 of 2 copies");
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
});

it("reports zero-copy failures with the actionable error, not partial success", async () => {
  const user = userEvent.setup();
  render(
    <BackgroundOperations>
      <SourceGroupCard
        source={healthySource}
        skills={skills}
        onCopyMember={async () => {
          throw new Error("Choose an empty destination.");
        }}
        pickDirectory={async () => "/tmp/copies"}
      />
    </BackgroundOperations>,
  );
  await user.click(screen.getByRole("checkbox", { name: "skills/alpha" }));
  await user.click(screen.getByRole("button", { name: "Create local copy" }));
  const notice = await screen.findByRole("region", {
    name: "Current activity",
  });
  expect(notice).toHaveTextContent("Choose an empty destination.");
  expect(notice.querySelector('[data-state="failed"]')).not.toBeNull();
  expect(screen.getByRole("checkbox", { name: "skills/alpha" })).toBeChecked();
});
