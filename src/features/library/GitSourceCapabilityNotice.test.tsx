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

test("renders every typed source state and closes only legacy/conflicted source actions", () => {
  render(<GitSourceCapabilityNotice report={report} />);

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
      "Source-level update and promotion are unavailable for this source.",
    ),
  ).toHaveLength(2);
});
