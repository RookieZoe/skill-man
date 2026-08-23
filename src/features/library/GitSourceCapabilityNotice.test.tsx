import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";

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

test("renders every typed source state without promising unavailable follow-up actions", () => {
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
      "A complete Source Release is recognized. Source actions are not available in this release.",
    ),
  ).toHaveLength(1);
  expect(
    screen.getByText(
      "Reading, Disable, and Remove remain available. Source-level Fetch Latest, Update, and Promotion are closed.",
    ),
  ).toBeInTheDocument();
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
