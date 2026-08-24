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
    },
    {
      remoteId: "legacy-source",
      canonicalUrl: "https://github.com/acme/legacy",
      kind: "legacy_per_skill_git_state",
    },
    {
      remoteId: "conflicted-source",
      canonicalUrl: "https://github.com/acme/conflicted",
      kind: "remote_source_identity_conflict",
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
    screen.getAllByText(
      "A complete Source Release is recognized. Fetch a fresh complete release before updating this source.",
    ),
  ).toHaveLength(1);
  expect(
    screen.getByText(
      "Reading, Disable, and Remove remain available. Promote this complete Legacy Source after reviewing a fresh Source Group Draft.",
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
