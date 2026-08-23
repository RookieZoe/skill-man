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
    trackingRef: "main",
    resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
    members: [
      {
        directoryName: "root",
        displayName: "Root Skill",
        description: "The repository root member",
        skillPath: "",
        treeSummary: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      },
      {
        directoryName: "nested",
        displayName: "Nested Skill",
        description: "A nested member",
        skillPath: "packages/nested",
        treeSummary: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      },
    ],
    externalOwnershipClaims: [
      {
        lockPath: "/Users/example/.skill-lock.json",
        entryName: "legacy-root",
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
      trackingRef=""
      outcome={outcome}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onTrackingRefChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={vi.fn()}
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
    screen.getByRole("button", { name: "Confirm complete Source Release" }),
  ).toBeInTheDocument();
});

test("offers only whole-source Undo in the completed result window", async () => {
  const user = userEvent.setup();
  const onUndo = vi.fn();
  render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl="https://github.com/acme/repository"
      trackingRef="main"
      outcome={preview}
      result={{
        operationId: "source-transition-1",
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
      onTrackingRefChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={vi.fn()}
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
  const onTrackingRefChange = vi.fn();
  const onFetch = vi.fn();
  const { rerender } = render(
    <SourceGroupPreviewFlow
      sourceType="git"
      sourceUrl="https://example.com/acme/repository.git"
      trackingRef=""
      outcome={{
        kind: "repository_ref_conflict",
        conflict: {
          provider: "git",
          sourceUrl: "https://example.com/acme/repository.git",
          availableRefs: ["main", "release"],
          externalOwnershipClaims: [],
        },
      }}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onTrackingRefChange={onTrackingRefChange}
      onFetch={onFetch}
      onConfirm={vi.fn()}
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
      trackingRef="release"
      outcome={{
        kind: "repository_ref_conflict",
        conflict: {
          provider: "git",
          sourceUrl: "https://example.com/acme/repository.git",
          availableRefs: ["main", "release"],
          externalOwnershipClaims: [],
        },
      }}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onTrackingRefChange={onTrackingRefChange}
      onFetch={onFetch}
      onConfirm={vi.fn()}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await user.click(screen.getByRole("button", { name: "Fetch selected ref" }));

  expect(onTrackingRefChange).toHaveBeenCalledWith("release");
  expect(onFetch).toHaveBeenCalledOnce();
});
