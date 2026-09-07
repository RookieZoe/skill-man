import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { GitSourceCapabilityReport } from "../../app/catalog-client";
import { GitSourceCapabilityNotice } from "./GitSourceCapabilityNotice";

const report: GitSourceCapabilityReport = {
  sources: [
    {
      remoteId: "current-source",
      canonicalUrl: "https://github.com/acme/current",
      kind: "git_repository_source",
      members: [],
    },
    {
      remoteId: "legacy-source",
      canonicalUrl: "https://github.com/acme/legacy",
      kind: "legacy_per_skill_git_state",
      members: [],
    },
    {
      remoteId: "conflicted-source",
      canonicalUrl: "https://github.com/acme/conflicted",
      kind: "remote_source_identity_conflict",
      members: [],
    },
  ],
};

test("renders every typed source state without opening source actions by default", () => {
  render(<GitSourceCapabilityNotice report={report} failure={null} />);

  expect(
    screen.getByRole("region", { name: "Git source status" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Git Repository Source")).toBeInTheDocument();
  expect(screen.getByText("Legacy Per-Skill Git State")).toBeInTheDocument();
  expect(
    screen.getByText("Remote Source Identity Conflict"),
  ).toBeInTheDocument();
  expect(
    screen.getByText("https://github.com/acme/conflicted"),
  ).toBeInTheDocument();
  expect(
    screen.getAllByText("Skills in this repository are updated together."),
  ).toHaveLength(1);
  expect(
    screen.getByText(
      "Upgrade this source to repository management first. You can still view, disable, or remove its Skills.",
    ),
  ).toBeInTheDocument();
});

test("offers source-scoped actions only for the matching typed source state", async () => {
  const onPromote = vi.fn();
  const onUpdate = vi.fn();
  const user = userEvent.setup();
  render(
    <GitSourceCapabilityNotice
      report={report}
      failure={null}
      onPromote={onPromote}
      onUpdate={onUpdate}
    />,
  );

  await user.click(
    screen.getByRole("button", { name: "Promote Legacy Source" }),
  );

  expect(onPromote).toHaveBeenCalledWith(
    "legacy-source",
    expect.any(HTMLButtonElement),
  );
  await user.click(screen.getByRole("button", { name: "Update Source" }));
  expect(onUpdate).toHaveBeenCalledWith(
    "current-source",
    expect.any(HTMLButtonElement),
  );
  expect(screen.getAllByRole("button")).toHaveLength(2);
});

test("surfaces a localized scan failure and keeps its raw diagnostic collapsed", () => {
  render(
    <GitSourceCapabilityNotice
      report={null}
      failure={{
        diagnostic: "git_source_capability_scan_failed: unreadable catalog",
      }}
    />,
  );

  expect(screen.getByRole("alert")).toHaveTextContent(
    "Git source status unavailable",
  );
  expect(screen.getByText("Technical details")).toBeInTheDocument();
  expect(
    screen
      .getByText("git_source_capability_scan_failed: unreadable catalog")
      .closest("details"),
  ).not.toHaveAttribute("open");
});

test("Restore removes the whole-source bytes only after an inline confirm", async () => {
  const onRestore = vi.fn();
  const user = userEvent.setup();
  render(
    <GitSourceCapabilityNotice
      report={report}
      failure={null}
      onRestore={onRestore}
    />,
  );

  await user.click(screen.getByRole("button", { name: "Restore Release" }));
  expect(onRestore).not.toHaveBeenCalled();
  await user.click(
    screen.getByRole("button", {
      name: "Restore Skill files from the current Source Release? Local changes will be overwritten.",
    }),
  );
  expect(onRestore).toHaveBeenCalledWith("current-source");
});

test("Remove Source requires an explicit inline confirm and can be cancelled", async () => {
  const onRemove = vi.fn();
  const user = userEvent.setup();
  render(
    <GitSourceCapabilityNotice
      report={report}
      failure={null}
      onRemove={onRemove}
    />,
  );

  await user.click(screen.getByRole("button", { name: "Remove Source" }));
  expect(onRemove).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onRemove).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("button", {
      name: "Remove this repository, its Skills, and their activations? This cannot be undone.",
    }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Remove Source" }));
  await user.click(
    screen.getByRole("button", {
      name: "Remove this repository, its Skills, and their activations? This cannot be undone.",
    }),
  );
  expect(onRemove).toHaveBeenCalledWith("current-source");
});

test("Copy Out forwards the single current member and picked destination without a picker list", async () => {
  const onCopyMember = vi.fn();
  const user = userEvent.setup();
  const sourceReport: GitSourceCapabilityReport = {
    sources: [
      {
        remoteId: "current-source",
        canonicalUrl: "https://github.com/acme/current",
        kind: "git_repository_source",
        members: [
          {
            skillId: "skill-a",
            skillPath: "skills/alpha",
            presence: true,
          },
          {
            skillId: "skill-b",
            skillPath: "skills/beta",
            presence: false,
          },
        ],
      },
    ],
  };
  render(
    <GitSourceCapabilityNotice
      report={sourceReport}
      failure={null}
      onCopyMember={onCopyMember}
      pickDirectory={async () => "/Users/zoe/export"}
    />,
  );

  expect(screen.queryByRole("combobox")).toBeNull();
  await user.click(screen.getByRole("button", { name: "Copy Out" }));
  expect(onCopyMember).toHaveBeenCalledWith(
    "current-source",
    "skill-a",
    "/Users/zoe/export",
  );
});

test("Copy Out offers a member picker when several current members exist", async () => {
  const onCopyMember = vi.fn();
  const user = userEvent.setup();
  const sourceReport: GitSourceCapabilityReport = {
    sources: [
      {
        remoteId: "current-source",
        canonicalUrl: "https://github.com/acme/current",
        kind: "git_repository_source",
        members: [
          {
            skillId: "skill-a",
            skillPath: "skills/alpha",
            presence: true,
          },
          {
            skillId: "skill-b",
            skillPath: "skills/beta",
            presence: true,
          },
        ],
      },
    ],
  };
  render(
    <GitSourceCapabilityNotice
      report={sourceReport}
      failure={null}
      onCopyMember={onCopyMember}
      pickDirectory={async () => "/Users/zoe/export"}
    />,
  );

  await user.selectOptions(screen.getByRole("combobox", { name: /Member/ }), [
    "skill-b",
  ]);
  await user.click(screen.getByRole("button", { name: "Copy Out" }));
  expect(onCopyMember).toHaveBeenCalledWith(
    "current-source",
    "skill-b",
    "/Users/zoe/export",
  );
});

test("Copy Out stays hidden when every member is tombstoned", () => {
  const sourceReport: GitSourceCapabilityReport = {
    sources: [
      {
        remoteId: "current-source",
        canonicalUrl: "https://github.com/acme/current",
        kind: "git_repository_source",
        members: [
          {
            skillId: "skill-b",
            skillPath: "skills/beta",
            presence: false,
          },
        ],
      },
    ],
  };
  render(
    <GitSourceCapabilityNotice
      report={sourceReport}
      failure={null}
      onCopyMember={vi.fn()}
    />,
  );

  expect(screen.queryByRole("button", { name: "Copy Out" })).toBeNull();
});
