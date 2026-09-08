import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import type {
  SourceGroupPreviewOutcome,
  SourceUpdateDraft,
} from "../../app/catalog-client";
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

function renderFlow(
  outcome: SourceGroupPreviewOutcome | null,
  activity: "idle" | "fetching" | "confirming" = "idle",
  sourceUrl = "https://github.com/acme/repository",
) {
  return render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl={sourceUrl}
      policyMode="branch"
      policyValue="main"
      outcome={outcome}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={null}
      error={null}
      activity={activity}
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

test("GitHub shorthand is valid in the native input and enables fetching", () => {
  renderFlow(null, "idle", "RookieZoe/skill-man");
  const input = screen.getByDisplayValue(
    "RookieZoe/skill-man",
  ) as HTMLInputElement;
  expect(input).toHaveAttribute("type", "text");
  expect(input.checkValidity()).toBe(true);
  expect(input).toHaveAttribute("aria-invalid", "false");
  expect(screen.getByRole("button", { name: /fetch.*preview/i })).toBeEnabled();
});

test("removed external members require explicit force confirmation and new members stay disabled", async () => {
  const user = userEvent.setup();
  const onConfirm = vi.fn();
  const changed: SourceGroupPreviewOutcome = {
    ...preview,
    preview: {
      ...preview.preview,
      removedExternalClaims: ["design"],
      addedMemberNames: ["ui"],
    },
  };
  render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl={changed.preview.sourceUrl}
      policyMode="branch"
      policyValue="main"
      outcome={changed}
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
  expect(screen.getByText("design")).toBeVisible();
  expect(screen.getByText("ui")).toBeVisible();
  expect(
    screen.getByText(/new Skills will not be distributed automatically/i),
  ).toBeVisible();
  expect(
    screen.queryByRole("button", { name: "Use latest remote release" }),
  ).not.toBeInTheDocument();
  const force = screen.getByRole("button", {
    name: "Force remote replacement",
  });
  expect(force).toBeDisabled();
  await user.click(
    screen.getByRole("checkbox", { name: /remove the listed Skills/i }),
  );
  await user.click(force);
  expect(onConfirm).toHaveBeenCalledExactlyOnceWith(["design"]);
});

test("renders the complete source group without per-member Include controls", () => {
  renderFlow(preview);

  expect(screen.getByText("Complete Source Release")).toBeInTheDocument();
  expect(screen.getByText("Provider")).toBeInTheDocument();
  expect(screen.getByText("github")).toBeInTheDocument();
  expect(screen.getByText("Root Skill")).toBeInTheDocument();
  expect(screen.getByText("Nested Skill")).toBeInTheDocument();
  expect(screen.getAllByText("Target tree")).toHaveLength(2);
  expect(screen.getByText("Existing installation records")).toBeInTheDocument();
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
    screen.getByRole("button", { name: "Install all 2 Skills" }),
  ).toBeEnabled();
});

test("leaves foreground progress to the enclosing import sheet", () => {
  const fetching = renderFlow(null, "fetching");
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  fetching.unmount();
  renderFlow(preview, "confirming");
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
});

test("collapses technical details by default", () => {
  const { container } = renderFlow(preview);
  expect(container.querySelectorAll("details")).toHaveLength(3);
  for (const details of container.querySelectorAll("details")) {
    expect(details.open).toBe(false);
  }
  expect(screen.queryByText("auto_release_tag_head")).not.toBeInTheDocument();
});

test("allows complete installation with partial external ownership claims", async () => {
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

  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  const replacement = screen.getByRole("button", {
    name: "Use latest remote release",
  });
  expect(replacement).toBeEnabled();
  await user.click(replacement);
  expect(onConfirm).toHaveBeenCalledOnce();
});

test.each([true, false])(
  "result shows errors and only promises Undo when available: %s",
  async (undoAvailable) => {
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
          undoAvailable,
        }}
        error="Could not finalize the operation"
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
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not finalize the operation",
    );
    if (undoAvailable) {
      expect(screen.getByText(/You can undo this source change/)).toBeVisible();
      await user.click(screen.getByRole("button", { name: "Source Undo" }));
      expect(onUndo).toHaveBeenCalledOnce();
    } else {
      expect(
        screen.queryByText(/You can undo this source change/),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "Source Undo" }),
      ).not.toBeInTheDocument();
    }
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  },
);

test("Source Update preview exposes confirm and keeps the result undoable", async () => {
  const user = userEvent.setup();
  const onConfirm = vi.fn();
  const updateDraft: SourceUpdateDraft = {
    remoteId: "remote-1",
    provider: "gitlab",
    sourceUrl: "https://gitlab.com/acme/repository",
    aliases: [],
    policy: preview.preview.policy,
    members: [
      {
        skillId: "skill-1",
        skillPath: "skills/root",
        directoryName: "root",
        directoryIdentityKey: "root",
        displayName: "Root Skill",
        description: "Updated member",
        treeSummary: "cccccccccccccccccccccccccccccccccccccccc",
        state: "current",
      },
    ],
  };
  const { rerender } = render(
    <SourceGroupPreviewFlow
      sourceType="github"
      sourceUrl={updateDraft.sourceUrl}
      policyMode="branch"
      policyValue="main"
      outcome={null}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={updateDraft}
      result={null}
      error={null}
      activity="idle"
      onSourceTypeChange={vi.fn()}
      onSourceUrlChange={vi.fn()}
      onSourceGroupPolicyChange={vi.fn()}
      onFetch={vi.fn()}
      onConfirm={vi.fn()}
      onConfirmPromotion={onConfirm}
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );

  expect(
    screen.getByRole("button", { name: "Update complete source" }),
  ).toBeInTheDocument();
  await user.click(
    screen.getByRole("button", { name: "Update complete source" }),
  );
  expect(onConfirm).toHaveBeenCalledOnce();

  rerender(
    <SourceGroupPreviewFlow
      sourceType="gitlab"
      sourceUrl={updateDraft.sourceUrl}
      policyMode="branch"
      policyValue="main"
      outcome={null}
      promotionDraft={null}
      promotionOutcome={null}
      updateDraft={null}
      result={{
        operationId: "update-operation-1",
        remoteId: "remote-1",
        releaseId: "source-release-2",
        resolvedCommit: "0123456789abcdef0123456789abcdef01234567",
        memberCount: 1,
        snapshotVersion: 10,
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
      onUndo={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "Source Undo" })).toBeEnabled();
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
