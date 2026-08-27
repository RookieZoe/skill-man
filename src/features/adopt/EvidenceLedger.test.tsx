import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";

import "../../styles.css";
import { createFixtureCatalogClient } from "../../test-fixtures/catalog";
import type {
  AdoptEvidenceCandidate,
  AdoptEvidenceReport,
  AdoptPlan,
} from "../../app/catalog-client";
import { LocaleProvider } from "../locale/LocaleProvider";
import { EvidenceLedger } from "./EvidenceLedger";

function candidate(
  overrides: Partial<AdoptEvidenceCandidate> = {},
): AdoptEvidenceCandidate {
  const entity = "~/.agents/skills/networking";
  const directoryName = "networking";
  return {
    canonicalEntity: entity,
    directoryName,
    directoryNames: [directoryName],
    appearances: [
      {
        entryPath: "~/.claude/skills/networking",
        kind: "symlink",
        agentId: "claude-code",
        shared: false,
        originalTarget: "../../.agents/skills/networking",
        chain: {
          entryPath: "~/.claude/skills/networking",
          entryDevice: 1,
          entryInode: 10,
          hops: [
            {
              path: "~/.claude/skills/networking",
              kind: "symlink",
              target: "../../.agents/skills/networking",
              device: 1,
              inode: 10,
            },
          ],
          finalEntity: entity,
          fault: null,
        },
      },
    ],
    verdict: "local",
    reason: { kind: "no_lock" },
    lock: null,
    remote: null,
    localTreeHash: "tree-sha256-v1:abc123",
    requiresRelocation: false,
    selectable: true,
    adoptable: true,
    conflict: null,
    suggestedAgentIds: [],
    ...overrides,
  };
}

function report(candidates: AdoptEvidenceCandidate[]): AdoptEvidenceReport {
  return {
    generation: 3,
    truncated: false,
    lockFiles: [],
    candidates,
    gitSources: [],
  };
}

function noop() {}

function gitLock(entryName: string) {
  return {
    lockPath: "~/.agents/.skill-lock.json",
    lockFingerprint: "lock-fingerprint",
    entryName,
    entry: {
      name: entryName,
      sourceType: "github",
      source: "https://github.com/acme/skills",
      sourceUrl: "https://github.com/acme/skills",
      requestedRef: "main",
      skillPath: `skills/${entryName}`,
      skillFolderHash: "a".repeat(40),
      installedAt: null,
      updatedAt: null,
      pluginName: null,
    },
    entryFault: null,
    fileFault: null,
  };
}

function renderLedger(
  reportValue: AdoptEvidenceReport,
  selections: Record<string, never> = {},
  plan: AdoptPlan | null = null,
  activity: "idle" | "scanning" | "planning" | "applying" | "undoing" = "idle",
) {
  return render(
    <EvidenceLedger
      report={reportValue}
      selections={selections}
      plan={plan}
      result={null}
      undo={null}
      error={null}
      errorHeading="app.notice.scan_failed"
      activity={activity}
      onToggle={noop}
      onSetBranch={noop}
      onRescan={noop}
      onPlan={noop}
      onApply={noop}
      onUndo={noop}
      onClose={noop}
    />,
  );
}

test("shows an indeterminate progress bar while rescanning", () => {
  renderLedger(report([candidate()]), {}, null, "scanning");

  expect(screen.getByRole("progressbar", { name: "Scanning" })).toHaveAttribute(
    "aria-valuetext",
    "Scanning",
  );
});

test("renders closed accordions and reveals the evidence on demand", async () => {
  renderLedger(
    report([
      candidate({
        lock: {
          lockPath: "~/.agents/.skill-lock.json",
          lockFingerprint: "deadbeef",
          entryName: "networking",
          entry: {
            name: "networking",
            sourceType: "github",
            source: "acme/networking",
            sourceUrl: "https://github.com/acme/networking",
            requestedRef: null,
            skillPath: "skills/networking",
            skillFolderHash: "0123456789abcdef0123456789abcdef01234567",
            installedAt: "2026-08-01T00:00:00Z",
            updatedAt: "2026-08-01T00:00:00Z",
            pluginName: null,
          },
          entryFault: null,
          fileFault: null,
        },
      }),
    ]),
  );
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  const accordion = dialog.querySelector(".adopt-ledger-candidate");
  expect(accordion).toBeInTheDocument();
  const disclosure = within(accordion as HTMLElement).getByRole("button", {
    name: /networking/,
  });
  expect(disclosure).toHaveAttribute("aria-expanded", "false");
  expect(disclosure).toHaveTextContent("networking");
  expect(
    disclosure.querySelector(".adopt-ledger-summary-content"),
  ).toBeInTheDocument();
  const details = (accordion as HTMLElement).querySelector(
    ".adopt-ledger-details",
  ) as HTMLElement;
  expect(details).toHaveAttribute("hidden");
  expect(getComputedStyle(details).display).toBe("none");

  await userEvent.click(disclosure);
  expect(disclosure).toHaveAttribute("aria-expanded", "true");
  expect(details).not.toHaveAttribute("hidden");
  expect(getComputedStyle(details).display).toBe("grid");
  expect(getComputedStyle(details).gridTemplateColumns).toBe("1fr");
  // Every hop is visible after the candidate is opened.
  expect(dialog).toHaveTextContent("hop 1");
  expect(dialog).toHaveTextContent("~/.claude/skills/networking");
  expect(dialog).toHaveTextContent("../../.agents/skills/networking");
  expect(dialog).toHaveTextContent("final entity");
  // Lock evidence renders raw Source Content verbatim, including the hit
  // path (spec §8.1).
  expect(dialog).toHaveTextContent("~/.agents/.skill-lock.json");
  expect(dialog).toHaveTextContent("https://github.com/acme/networking");
  expect(dialog).toHaveTextContent("0123456789abcdef0123456789abcdef01234567");
  expect(dialog).toHaveTextContent("deadbeef");
  // The verdict column shows the closed verdict and the plan result.
  expect(dialog).toHaveTextContent("Local Source");
  expect(dialog).toHaveTextContent("No lock declaration");
  // Include control exists for selectable candidates.
  expect(
    within(dialog).getByRole("checkbox", { name: /Include/ }),
  ).toBeInTheDocument();
  expect(
    within(dialog)
      .getByRole("checkbox", { name: /Include/ })
      .closest(".adopt-ledger-summary"),
  ).toBeInTheDocument();
});

test("uses the skill name color to signal an abnormal candidate", () => {
  renderLedger(
    report([
      candidate({
        directoryName: "provenance-conflict",
        canonicalEntity: "~/.agents/skills/provenance-conflict",
        verdict: "conflict",
        reason: { kind: "ownership_conflict", managedDirectoryName: "managed" },
        selectable: false,
        adoptable: false,
      }),
    ]),
  );

  const name = screen.getByText("provenance-conflict");
  expect(name).toHaveClass("adopt-verdict-conflict");
  expect(name.closest(".adopt-ledger-candidate")).not.toHaveClass(
    "adopt-ledger-candidate--expanded",
  );
});

test("keeps normal skill names in the default text color", () => {
  renderLedger(report([candidate()]));

  expect(screen.getByText("networking")).toHaveClass("adopt-candidate-name");
  expect(screen.getByText("networking")).not.toHaveClass("adopt-verdict-local");
});

test("keeps collapsed candidates from shrinking inside the scrolling ledger body", () => {
  renderLedger(report([candidate()]));

  const body = document.querySelector(".adopt-ledger-body") as HTMLElement;
  const card = document.querySelector(".adopt-ledger-candidate") as HTMLElement;

  expect(getComputedStyle(body).display).toBe("flex");
  expect(getComputedStyle(body).flexDirection).toBe("column");
  expect(getComputedStyle(card).overflow).toBe("hidden");
  expect(getComputedStyle(card).flexShrink).toBe("0");
});

test("ships an explicit hidden-details rule for the native WebKit accordion", () => {
  const rule = Array.from(document.styleSheets)
    .flatMap((sheet) => Array.from(sheet.cssRules))
    .find(
      (candidate): candidate is CSSStyleRule =>
        candidate instanceof CSSStyleRule &&
        candidate.selectorText === ".adopt-ledger-details[hidden]",
    );

  expect(rule?.style.getPropertyValue("display")).toBe("none");
  expect(rule?.style.getPropertyPriority("display")).toBe("important");
});

test("does not scale the accordion disclosure while it is pressed", () => {
  const rule = Array.from(document.styleSheets)
    .flatMap((sheet) => Array.from(sheet.cssRules))
    .find(
      (candidate): candidate is CSSStyleRule =>
        candidate instanceof CSSStyleRule &&
        candidate.selectorText ===
          ".adopt-ledger-disclosure:active:not(:disabled)",
    );

  expect(rule?.style.getPropertyValue("transform")).toBe("none");
});

test("keeps the title Include control independent from disclosure", async () => {
  const onToggle = vi.fn();
  const user = userEvent.setup();
  render(
    <EvidenceLedger
      report={report([candidate()])}
      selections={{}}
      plan={null}
      result={null}
      undo={null}
      error={null}
      errorHeading="app.notice.scan_failed"
      activity="idle"
      onToggle={onToggle}
      onSetBranch={noop}
      onRescan={noop}
      onPlan={noop}
      onApply={noop}
      onUndo={noop}
      onClose={noop}
    />,
  );

  const disclosure = screen.getByRole("button", { name: /networking/ });
  const checkbox = screen.getByRole("checkbox", { name: /Include/ });
  await user.click(checkbox);

  expect(onToggle).toHaveBeenCalledWith("~/.agents/skills/networking", true);
  expect(disclosure).toHaveAttribute("aria-expanded", "false");
});

test("shows disabled Include controls for non-selectable candidates", () => {
  renderLedger(
    report([
      candidate(),
      candidate({
        canonicalEntity: "~/.agents/skills/broken",
        directoryName: "broken",
        directoryNames: ["broken"],
        appearances: [],
        verdict: "blocked",
        reason: {
          kind: "chain_fault",
          fault: { kind: "dangling", at: "~/.agents/skills/broken" },
        },
        selectable: false,
        adoptable: false,
      }),
      candidate({
        canonicalEntity: "~/.agents/skills/deferred",
        directoryName: "deferred",
        directoryNames: ["deferred"],
        appearances: [],
        verdict: "deferred",
        reason: {
          kind: "remote_unavailable",
          detail: "Could not resolve host",
        },
        selectable: false,
        adoptable: false,
      }),
      candidate({
        canonicalEntity: "~/.agents/skills/fixture",
        directoryName: "fixture",
        directoryNames: ["fixture"],
        appearances: [],
        verdict: "excluded",
        reason: { kind: "fixture_entity" },
        selectable: false,
        adoptable: false,
      }),
    ]),
  );
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  const checkboxes = within(dialog).getAllByRole("checkbox");
  expect(checkboxes).toHaveLength(4);
  expect(checkboxes[0]).toBeEnabled();
  for (const checkbox of checkboxes.slice(1)) {
    expect(checkbox).toBeDisabled();
  }
  expect(dialog).not.toHaveTextContent("No selection control");
});

test("shows the three Modified branches simultaneously and requires a branch", async () => {
  const entity = "~/.agents/skills/modified";
  const modified = candidate({
    canonicalEntity: entity,
    directoryName: "modified",
    directoryNames: ["modified"],
    verdict: "modified",
    reason: null,
    remote: {
      canonicalUrl: "https://github.com/acme/networking",
      requestedRef: "HEAD",
      refKind: "head",
      anchorCommit: "c".repeat(40),
      originalInstallCommitKnown: false,
      skillPath: "skills/networking",
      providerHash: "0".repeat(40),
      providerHashMatched: true,
      remoteTreeHash: "tree-sha256-v1:remote",
      localTreeHash: "tree-sha256-v1:local",
      treesMatch: false,
      defaultBranch: "main",
    },
  });
  const { rerender } = renderLedger(report([modified]));
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  await userEvent.click(
    within(dialog).getByRole("button", { name: /modified/ }),
  );
  // All three branches are visible at once; keep-current is the
  // recommendation but the branch choice never includes the candidate.
  expect(
    within(dialog).getByRole("radio", { name: /Keep current bytes/ }),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByRole("radio", {
      name: /reinstall the Verification Anchor/,
    }),
  ).toBeInTheDocument();
  expect(
    within(dialog).getByRole("radio", { name: /Convert to Local Link/ }),
  ).toBeInTheDocument();
  expect(dialog).toHaveTextContent("Remote and local trees differ");
  expect(dialog).toHaveTextContent("tree-sha256-v1:remote");
  expect(dialog).toHaveTextContent("tree-sha256-v1:local");
  expect(dialog).toHaveTextContent("c".repeat(40));

  // Including the candidate selects keep-current by default.
  const { onToggle, onSetBranch } = { onToggle: noop, onSetBranch: noop };
  rerender(
    <EvidenceLedger
      report={report([modified])}
      selections={{
        [entity]: {
          canonicalEntity: entity,
          agentIds: [],
          modifiedBranch: "keep_current",
        },
      }}
      plan={null}
      result={null}
      undo={null}
      error={null}
      errorHeading="app.notice.scan_failed"
      activity="idle"
      onToggle={onToggle}
      onSetBranch={onSetBranch}
      onRescan={noop}
      onPlan={noop}
      onApply={noop}
      onUndo={noop}
      onClose={noop}
    />,
  );
  expect(
    within(dialog).getByRole("radio", { name: /Keep current bytes/ }),
  ).toBeChecked();
});

test("plan view shows the frozen intent and enables Apply for the handoff", () => {
  const plan: AdoptPlan = {
    planToken: "adopt-plan-1",
    evidenceGeneration: 3,
    items: [
      {
        directoryName: "networking",
        canonicalEntity: "~/.agents/skills/networking",
        intent: "remote_install_keep_current",
        finalEntityPath: "",
        appearances: [],
        targetAgents: [],
        applyable: true,
        error: null,
      },
    ],
    canApply: true,
  };
  renderLedger(report([candidate()]), {}, plan);
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  expect(dialog).toHaveTextContent("Remote Install — keep current bytes");
  expect(dialog).toHaveTextContent("Ready");
  expect(within(dialog).getByRole("button", { name: "Adopt" })).toBeEnabled();
});

test("ownership conflict reason renders as a closed conflict verdict", async () => {
  renderLedger(
    report([
      candidate({
        verdict: "conflict",
        reason: {
          kind: "ownership_conflict",
          managedDirectoryName: "networking",
        },
        selectable: false,
        adoptable: false,
      }),
    ]),
  );
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  await userEvent.click(
    within(dialog).getByRole("button", { name: /networking/ }),
  );
  expect(dialog).toHaveTextContent(
    "The external installer reappeared for this Managed Skill",
  );
  expect(dialog).toHaveTextContent("Conflict");
  expect(
    within(dialog).getByRole("checkbox", { name: /Include/ }),
  ).toBeDisabled();
});

test("groups supported Git candidates by repository and starts fresh source management", async () => {
  const user = userEvent.setup();
  const onManageGitSource = vi.fn();
  const alphaLock = gitLock("alpha");
  const gitReport: AdoptEvidenceReport = {
    ...report([
      candidate({
        directoryName: "alpha",
        directoryNames: ["alpha"],
        lock: {
          ...alphaLock,
          entry: {
            ...alphaLock.entry,
            sourceUrl: "https://github.com/acme/skills.git",
          },
        },
      }),
    ]),
    gitSources: [
      {
        sourceType: "github",
        sourceUrl: "https://github.com/acme/skills",
        trackingRefs: ["main"],
        externalOwnershipClaims: [
          {
            lockPath: "~/.agents/.skill-lock.json",
            entryName: "alpha",
            requestedRef: "main",
          },
          {
            lockPath: "~/.agents/.skill-lock.json",
            entryName: "beta",
            requestedRef: "main",
          },
        ],
      },
    ],
  };
  render(
    <EvidenceLedger
      report={gitReport}
      selections={{}}
      plan={null}
      result={null}
      undo={null}
      error={null}
      errorHeading="app.notice.scan_failed"
      activity="idle"
      onToggle={noop}
      onSetBranch={noop}
      onRescan={noop}
      onPlan={noop}
      onApply={noop}
      onUndo={noop}
      onClose={noop}
      onManageGitSource={onManageGitSource}
    />,
  );

  expect(
    screen.getByRole("heading", {
      name: "Git Repository Source https://github.com/acme/skills",
    }),
  ).toBeInTheDocument();
  expect(screen.getByText("alpha")).toBeInTheDocument();
  expect(screen.getByText("beta")).toBeInTheDocument();
  expect(
    screen.queryByRole("checkbox", { name: "Include" }),
  ).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Install Git Skills" }));
  expect(onManageGitSource).toHaveBeenCalledWith(gitReport.gitSources[0]);
});

test("source content stays byte-identical in both locales", async () => {
  const client = createFixtureCatalogClient();
  client.getLocaleSnapshot = async () => ({
    selection: "zh-Hans",
    effectiveLocale: "zh-Hans",
    generation: 1,
    diagnostic: null,
  });
  client.setLocaleSelection = async (selection) => ({
    selection,
    effectiveLocale: selection === "system" ? "en" : selection,
    generation: 2,
    diagnostic: null,
  });
  const entity = "~/.agents/skills/networking";
  const rendered = render(
    <LocaleProvider client={client}>
      <EvidenceLedger
        report={report([
          candidate({
            verdict: "verified",
            remote: {
              canonicalUrl: "https://github.com/acme/networking",
              requestedRef: "HEAD",
              refKind: "head",
              anchorCommit: "c".repeat(40),
              originalInstallCommitKnown: false,
              skillPath: "skills/networking",
              providerHash: "0".repeat(40),
              providerHashMatched: true,
              remoteTreeHash: "tree-sha256-v1:remote",
              localTreeHash: "tree-sha256-v1:remote",
              treesMatch: true,
              defaultBranch: "main",
            },
          }),
        ])}
        selections={{}}
        plan={null}
        result={null}
        undo={null}
        error={null}
        errorHeading="app.notice.scan_failed"
        activity="idle"
        onToggle={noop}
        onSetBranch={noop}
        onRescan={noop}
        onPlan={noop}
        onApply={noop}
        onUndo={noop}
        onClose={noop}
      />
    </LocaleProvider>,
  );
  const dialog = await within(rendered.container).findByRole("dialog", {
    name: "纳管未纳管技能",
  });
  await userEvent.click(
    within(dialog).getByRole("button", { name: /networking/ }),
  );
  // App Copy is localized; Source Content is untouched.
  expect(dialog).toHaveTextContent("已验证远程来源");
  expect(dialog).toHaveTextContent("https://github.com/acme/networking");
  expect(dialog).toHaveTextContent("tree-sha256-v1:remote");
  expect(dialog).toHaveTextContent("c".repeat(40));
  expect(dialog).toHaveTextContent(entity);
});
