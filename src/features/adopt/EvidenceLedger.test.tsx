import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";

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
              kind: "symlink:../../.agents/skills/networking",
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
  return { generation: 3, truncated: false, lockFiles: [], candidates };
}

function noop() {}

function renderLedger(
  reportValue: AdoptEvidenceReport,
  selections: Record<string, never> = {},
  plan: AdoptPlan | null = null,
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
      activity="idle"
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

test("renders the three columns with the source chain fully expanded by default", () => {
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
  // Every hop is visible without any interaction (default fully expanded).
  expect(dialog).toHaveTextContent("hop 1");
  expect(dialog).toHaveTextContent("~/.claude/skills/networking");
  expect(dialog).toHaveTextContent("../../.agents/skills/networking");
  expect(dialog).toHaveTextContent("final entity");
  // Lock evidence renders raw Source Content verbatim.
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
});

test("shows Include controls only for selectable candidates", () => {
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
  expect(checkboxes).toHaveLength(1);
  expect(checkboxes[0].getAttribute("aria-label")).toBeNull();
  expect(dialog).toHaveTextContent("No selection control");
  expect(dialog).toHaveTextContent("Could not resolve host");
  expect(dialog).toHaveTextContent("Fixture footprint");
});

test("shows the three Modified branches simultaneously and requires a branch", () => {
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

test("plan view shows the frozen intent and keeps Apply disabled for handoff", () => {
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
        applyable: false,
        error: null,
      },
    ],
    canApply: false,
  };
  renderLedger(report([candidate()]), {}, plan);
  const dialog = screen.getByRole("dialog", { name: "Adopt untracked Skills" });
  expect(dialog).toHaveTextContent("Remote Install — keep current bytes");
  expect(dialog).toHaveTextContent("Frozen handoff intent");
  expect(
    within(dialog).getByRole("button", { name: "Adopt" }),
  ).toBeDisabled();
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
  // App Copy is localized; Source Content is untouched.
  expect(dialog).toHaveTextContent("已验证远程来源");
  expect(dialog).toHaveTextContent("https://github.com/acme/networking");
  expect(dialog).toHaveTextContent("tree-sha256-v1:remote");
  expect(dialog).toHaveTextContent("c".repeat(40));
  expect(dialog).toHaveTextContent(entity);
});
