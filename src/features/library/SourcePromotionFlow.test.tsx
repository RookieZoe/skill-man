import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { SourcePromotionDraft } from "../../app/catalog-client";
import { SourcePromotionFlow } from "./SourcePromotionFlow";

const draft: SourcePromotionDraft = {
  remoteId: "legacy-source",
  provider: "github",
  canonicalUrl: "https://github.com/acme/skills",
  trackingRef: "main",
  resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
  existingMembers: [
    {
      skillId: "alpha",
      directoryName: "alpha",
      skillPath: "skills/alpha",
      modified: true,
      state: "modified_member_resolution_required",
    },
    {
      skillId: "legacy-beta",
      directoryName: "beta",
      skillPath: "skills/old-beta",
      modified: true,
      state: "upstream_member_removed",
    },
  ],
  targetMembers: [
    {
      member: {
        directoryName: "alpha",
        displayName: "Alpha",
        description: "",
        skillPath: "skills/alpha",
        treeSummary: "a".repeat(40),
      },
      legacySkillId: "alpha",
    },
    {
      member: {
        directoryName: "beta-v2",
        displayName: "Beta v2",
        description: "",
        skillPath: "skills/beta-v2",
        treeSummary: "b".repeat(40),
      },
      legacySkillId: null,
    },
  ],
};

test("requires explicit, limited resolutions before confirming the whole Source Release", async () => {
  const user = userEvent.setup();
  const onConfirm = vi.fn();
  render(
    <SourcePromotionFlow
      draft={draft}
      result={null}
      error={null}
      activity="idle"
      onConfirm={onConfirm}
      onUndo={vi.fn()}
      onClose={vi.fn()}
      initialFocusRef={null}
    />,
  );

  const confirm = screen.getByRole("button", {
    name: "Confirm Source Promotion",
  });
  expect(confirm).toBeDisabled();
  expect(
    screen.queryByRole("button", { name: /include/i }),
  ).not.toBeInTheDocument();

  await user.click(screen.getByRole("radio", { name: "Replace with target" }));
  await user.click(
    screen.getByRole("radio", { name: "Explicit Member Mapping" }),
  );
  await user.selectOptions(
    screen.getByRole("combobox", { name: "Target skill path" }),
    "skills/beta-v2",
  );
  expect(confirm).toBeDisabled();

  await user.click(
    screen.getAllByRole("radio", { name: "Keep Modified" }).at(-1)!,
  );
  expect(confirm).toBeEnabled();
  await user.click(confirm);

  expect(onConfirm).toHaveBeenCalledWith([
    {
      skillId: "alpha",
      modified: "replace_with_target",
      removed: null,
    },
    {
      skillId: "legacy-beta",
      modified: "keep_modified",
      removed: {
        kind: "explicit_member_mapping",
        targetSkillPath: "skills/beta-v2",
      },
    },
  ]);
});

test("uses Source Update terminology when advancing a managed source", () => {
  render(
    <SourcePromotionFlow
      draft={draft}
      result={null}
      error={null}
      activity="idle"
      onConfirm={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
      initialFocusRef={null}
      mode="update"
    />,
  );

  expect(
    screen.getByRole("heading", { name: "Review Source Update" }),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Confirm Source Update" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("Legacy Source Promotion")).not.toBeInTheDocument();
});
