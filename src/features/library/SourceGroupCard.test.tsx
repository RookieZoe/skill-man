import { render, screen } from "@testing-library/react";
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
    expect(screen.getByText("Explicit override")).toBeInTheDocument();
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
      name: "Restore Current Source Release",
    });
    await user.click(restoreBtn);
    const confirmRestoreBtn = screen.getByRole("button", {
      name: "Restore release",
    });
    await user.click(confirmRestoreBtn);
    expect(onRestore).toHaveBeenCalledWith("source-1");

    // Local copy note is visible
    expect(
      screen.getByText("Creating a local copy does not switch any Activation."),
    ).toBeInTheDocument();

    // Create Local Source Copy
    const copyBtn = screen.getByRole("button", {
      name: "Create Local Source Copy",
    });
    await user.click(copyBtn);
    expect(onCopyMember).toHaveBeenCalledWith(
      "source-1",
      expect.any(String),
      "/Users/test/Desktop/copied",
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
        "Legacy sources must be promoted before source-level operations are available.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Update" }),
    ).not.toBeInTheDocument();

    rerender(<SourceGroupCard source={conflictSource} skills={[]} />);
    expect(
      screen.getByText(
        "Source identity conflict must be resolved before source-level operations are available.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Update" }),
    ).not.toBeInTheDocument();
  });
});
