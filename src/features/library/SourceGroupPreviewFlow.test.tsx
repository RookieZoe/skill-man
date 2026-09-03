import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type { SourceGroupPreviewOutcome } from "../../app/catalog-client";
import { SourceGroupPreviewFlow } from "./SourceGroupPreviewFlow";

const preview: SourceGroupPreviewOutcome = {
  kind: "preview",
  preview: {
    provider: "github",
    sourceUrl: "https://github.com/acme/repository",
    aliases: [],
    policy: {
      mode: "branch",
      value: "main",
      selectionKind: "branch",
      selectedRef: "main",
      resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
    },
    members: [
      {
        directoryName: "root",
        displayName: "Root Skill",
        description: "The repository root member",
        skillPath: "",
        treeSummary: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        action: "added",
      },
      {
        directoryName: "nested",
        displayName: "Nested Skill",
        description: "A nested member",
        skillPath: "packages/nested",
        treeSummary: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        action: "added",
      },
    ],
    externalOwnershipClaims: [
      {
        lockPath: "/Users/example/.skill-lock.json",
        entryName: "root",
        requestedRef: "main",
      },
      {
        lockPath: "/Users/example/.skill-lock.json",
        entryName: "nested",
        requestedRef: "main",
      },
    ],
  },
};

function renderFlow(outcome: SourceGroupPreviewOutcome | null) {
  return render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl="https://github.com/acme/repository"
      policyMode="branch"
      policyValue="main"
      outcome={outcome}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={vi.fn()}
      onConfirmPromotion={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );
}

test("renders the complete source group without per-member Include controls", () => {
  renderFlow(preview);

  expect(screen.getByText("Complete Source Release")).toBeInTheDocument();
  expect(screen.getByText("Provider")).toBeInTheDocument();
  expect(screen.getByText("github")).toBeInTheDocument();
  expect(screen.getByText("Root Skill")).toBeInTheDocument();
  expect(screen.getByText("Nested Skill")).toBeInTheDocument();
  expect(screen.getAllByText("Target tree")).toHaveLength(2);
  expect(screen.getByText("External Ownership Claims")).toBeInTheDocument();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: /include/i }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Use latest remote release" }),
  ).toBeEnabled();
});

test("allows a new Git source that has no external ownership claims", () => {
  const newSource: SourceGroupPreviewOutcome = {
    ...preview,
    preview: {
      ...preview.preview,
      externalOwnershipClaims: [],
    },
  };

  renderFlow(newSource);

  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Use latest remote release" }),
  ).toBeEnabled();
});

test("blocks replacement before confirmation when a source member has no external claim", async () => {
  const user = userEvent.setup();
  const onConfirm = vi.fn();
  const incomplete: SourceGroupPreviewOutcome = {
    ...preview,
    preview: {
      ...preview.preview,
      externalOwnershipClaims: [
        {
          lockPath: "/Users/example/.skill-lock.json",
          entryName: "root",
          requestedRef: "main",
        },
      ],
    },
  };

  render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl="https://github.com/acme/repository"
      policyMode="branch"
      policyValue="main"
      outcome={incomplete}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={onConfirm}
      onConfirmPromotion={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );

  expect(screen.getByRole("alert")).toHaveTextContent(
    "does not declare every Source Member",
  );
  const replacement = screen.getByRole("button", {
    name: "Use latest remote release",
  });
  expect(replacement).toBeDisabled();
  await user.click(replacement);
  expect(onConfirm).not.toHaveBeenCalled();
});

test("offers only whole-source Undo in the completed result window", async () => {
  const user = userEvent.setup();
  const onUndo = vi.fn();
  render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl="https://github.com/acme/repository"
      policyMode="branch"
      policyValue="main"
      outcome={preview}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={{
        operationId: "source-transition-1",
        remoteId: "remote-1",
        releaseId: "source-release-1",
        resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
        memberCount: 2,
        snapshotVersion: 9,
        undoAvailable: true,
      }}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={vi.fn()}
      onConfirmPromotion={vi.fn()}
      onUndo={onUndo}
      onClose={vi.fn()}
    />,
  );

  expect(
    screen.getByText("Whole Source Release is managed"),
  ).toBeInTheDocument();
  expect(screen.getByText("source-release-1")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Source Undo" }));
  expect(onUndo).toHaveBeenCalledOnce();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
});

test("requires a ref choice before re-fetching a typed ref conflict", async () => {
  const user = userEvent.setup();
  const onPolicyChange = vi.fn();
  const onFetch = vi.fn();
  const { rerender } = render(
    <SourceGroupPreviewFlow
      sourceType="git"
      sourceUrl="https://example.com/acme/repository.git"
      policyMode="branch"
      policyValue=""
      outcome={{
        kind: "repository_ref_conflict",
        conflict: {
          provider: "git",
          sourceUrl: "https://example.com/acme/repository.git",
          availableRefs: ["main", "release"],
          externalOwnershipClaims: [],
        },
      }}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={onPolicyChange}
      onFetch={onFetch}
      onConfirm={vi.fn()}
      onConfirmPromotion={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );

  await user.selectOptions(
    screen.getByRole("combobox", { name: "Tracking ref (optional)" }),
    "release",
  );
  rerender(
    <SourceGroupPreviewFlow
      sourceType="git"
      sourceUrl="https://example.com/acme/repository.git"
      policyMode="branch"
      policyValue="release"
      outcome={{
        kind: "repository_ref_conflict",
        conflict: {
          provider: "git",
          sourceUrl: "https://example.com/acme/repository.git",
          availableRefs: ["main", "release"],
          externalOwnershipClaims: [],
        },
      }}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={onPolicyChange}
      onFetch={onFetch}
      onConfirm={vi.fn()}
      onConfirmPromotion={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await user.click(screen.getByRole("button", { name: "Fetch selected ref" }));

  expect(onPolicyChange).toHaveBeenCalledWith("branch", "release");
  expect(onFetch).toHaveBeenCalledOnce();
});
