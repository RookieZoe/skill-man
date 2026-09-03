import { useEffect, useRef, useState } from "react";

import type {
  CatalogClient,
  CurrentReport,
  ObservationAndScanSnapshot,
  ScanOperationEligibility,
  ScanReportRow,
  ScanReportSection,
  ScanRunSnapshot,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import type { MessageKey, MessageParams } from "../locale/messages";

/**
 * Non-modal Evidence Ledger for the full Rescan (spec §4.10, ADR-0020) and
 * the pinned 扫描汇总 sheet (spec §7.6, ADR-0017/ADR-0021): the shared state
 * surface on Library Desk and Agent Management. It only shows honest facts —
 * state, phase, root, counts, elapsed, Slow — never a percent or an ETA.
 *
 * The terminal Report renders the §7.6 information structure: the funnel
 * counts, the four count cards, the Root coverage table (consumer Agents,
 * result and evidence summary) and the candidate list in the fixed §8.1
 * block order (Scan incomplete, Needs attention, Git sources, Local
 * sources, Excluded/already Managed). Every candidate row carries its typed
 * Core operation eligibility (§8.1): a destructive operation is shown
 * blocked with the same closed reason the Core reports (`scan_incomplete`)
 * while the keep-in-place non-destructive Local Link stays allowed. All
 * selections default empty; Blocked/Deferred rows have no selection control.
 */

const PAGE_SIZE = 64;

interface SectionState {
  rows: ScanReportRow[];
  nextOffset: number | null;
  loading: boolean;
  /** The section fetched at least one page for the current Report. */
  loaded: boolean;
}

const EMPTY_SECTIONS: Record<ScanReportSection, SectionState> = {
  roots: { rows: [], nextOffset: null, loading: false, loaded: false },
  entities: { rows: [], nextOffset: null, loading: false, loaded: false },
  appearances: { rows: [], nextOffset: null, loading: false, loaded: false },
  diagnostics: { rows: [], nextOffset: null, loading: false, loaded: false },
  git_sources: { rows: [], nextOffset: null, loading: false, loaded: false },
  local_candidates: {
    rows: [],
    nextOffset: null,
    loading: false,
    loaded: false,
  },
  conflict_sets: { rows: [], nextOffset: null, loading: false, loaded: false },
  needs_attention: {
    rows: [],
    nextOffset: null,
    loading: false,
    loaded: false,
  },
  excluded: { rows: [], nextOffset: null, loading: false, loaded: false },
};

const CANDIDATE_SECTIONS: ScanReportSection[] = [
  "needs_attention",
  "git_sources",
  "local_candidates",
  "excluded",
];

function stateKey(run: ScanRunSnapshot): MessageKey {
  switch (run.state) {
    case "queued":
      return "scan.ledger.queued";
    case "running":
      return "scan.ledger.running";
    case "cancelling":
      return "scan.ledger.cancelling";
    case "cancelled":
      return "scan.ledger.cancelled";
    case "superseded":
      return "scan.ledger.superseded";
    case "completed":
      return run.counts.failedRoots > 0
        ? "scan.ledger.incomplete"
        : "scan.ledger.completed";
    case "failed":
      return "scan.ledger.failed";
  }
}

function phaseKey(run: ScanRunSnapshot): MessageKey {
  return `scan.ledger.phase.${run.phase}` as MessageKey;
}

function triggerKey(run: ScanRunSnapshot): MessageKey {
  return run.trigger === "manual"
    ? "scan.ledger.triggerManual"
    : "scan.ledger.triggerOnboarding";
}

function reportStateKey(report: CurrentReport["summary"]): MessageKey {
  if (report === null) return "scan.ledger.none";
  return report.state === "complete"
    ? "scan.ledger.completed"
    : "scan.ledger.incomplete";
}

function sectionKey(section: ScanReportSection): MessageKey {
  switch (section) {
    case "roots":
      return "scan.ledger.sectionRoots";
    case "entities":
      return "scan.ledger.sectionEntities";
    case "appearances":
      return "scan.ledger.sectionAppearances";
    case "diagnostics":
      return "scan.ledger.sectionDiagnostics";
    case "git_sources":
      return "scan.summary.block.gitSources";
    case "local_candidates":
      return "scan.summary.block.localSources";
    case "conflict_sets":
      return "scan.summary.block.conflictSets";
    case "needs_attention":
      return "scan.summary.block.needsAttention";
    case "excluded":
      return "scan.summary.block.excluded";
  }
}

function verdictKey(verdict: string): MessageKey {
  return `scan.summary.verdict.${verdict}` as MessageKey;
}

function reasonKey(kind: string | null): MessageKey | null {
  if (!kind) return null;
  return `scan.summary.reason.${kind}` as MessageKey;
}

function noteKey(note: string): MessageKey {
  return `scan.summary.note.${note}` as MessageKey;
}

function statusKey(status: string): MessageKey {
  return `scan.summary.group.status.${status}` as MessageKey;
}

function operationKey(operation: string): MessageKey {
  return `scan.summary.op.${operation}` as MessageKey;
}

function rowText(
  row: ScanReportRow,
  t: (key: MessageKey, params?: MessageParams) => string,
): string {
  switch (row.kind) {
    case "root_coverage":
      return t("scan.ledger.rootRow", {
        path: row.canonicalPath,
        state: t(`scan.ledger.rootState.${row.state}` as MessageKey),
      });
    case "entity":
      return t("scan.ledger.entityRow", {
        seq: row.entitySeq,
        path: row.canonicalPath,
        appearances: row.appearances,
      });
    case "appearance":
      return t("scan.ledger.appearanceRow", {
        name: row.name,
        path: row.entryPath,
        entity: row.entitySeq ?? "-",
      });
    case "diagnostic":
      return t("scan.ledger.diagnosticRow", {
        kind: row.diagnosticKind,
        at: row.at ?? "-",
      });
    case "git_source_group":
      return t("scan.summary.group.row", {
        provider: row.provider,
        repository: row.canonicalRepository,
        members: row.memberNames.length,
        status: t(statusKey(row.status)),
      });
    case "source_verdict":
      return t("scan.summary.candidate.row", {
        name: row.directoryNames.join(", ") || row.canonicalPath,
        path: row.canonicalPath,
      });
    case "conflict_set":
      return t("scan.summary.conflict.row", {
        name: row.directoryName,
        members: row.memberPaths.length,
      });
  }
}

/** Selection draft identity of one candidate row (default empty). */
function candidateKey(row: ScanReportRow): string {
  switch (row.kind) {
    case "git_source_group":
      return `git:${row.groupSeq}`;
    case "source_verdict":
      return `verdict:${row.entitySeq}`;
    case "conflict_set":
      return `set:${row.setSeq}`;
    default:
      return "";
  }
}

function operationEligibility(row: ScanReportRow): ScanOperationEligibility[] {
  switch (row.kind) {
    case "git_source_group":
      return row.operations;
    case "source_verdict":
      return row.operations;
    default:
      return [];
  }
}

/**
 * The §7.6 count cards: Git source candidate (groups), Local, Conflict set,
 * Excluded·already Managed — counting only, never selection carriers.
 */
function countCards(
  summary: NonNullable<CurrentReport["summary"]>,
): Array<{ key: MessageKey; value: number }> {
  return [
    {
      key: "scan.summary.cards.gitCandidate",
      value: summary.sourceCounts.gitGroups,
    },
    {
      key: "scan.summary.cards.local",
      value:
        summary.sourceCounts.localCandidates -
        summary.sourceCounts.conflictMembers,
    },
    {
      key: "scan.summary.cards.conflictSet",
      value: summary.sourceCounts.conflictSets,
    },
    {
      key: "scan.summary.cards.excluded",
      value:
        summary.sourceCounts.excluded + summary.sourceCounts.alreadyManaged,
    },
  ];
}

export function ScanEvidenceLedger({
  client,
  idle = false,
}: {
  client: CatalogClient;
  /** The surface is not writable (ReadOnly/Closed gate): hide the actions. */
  idle?: boolean;
}) {
  const { t } = useLocale();
  const [observation, setObservation] =
    useState<ObservationAndScanSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sections, setSections] =
    useState<Record<ScanReportSection, SectionState>>(EMPTY_SECTIONS);
  const lastReportIdentity = useRef<string | null>(null);
  // §8.1: every candidate selection defaults empty; the draft is presentation
  // state only. Reset when the Report identity changes.
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  const [winners, setWinners] = useState<Record<string, number>>({});

  // Page sections are bound to one Report identity: a published new Report
  // resets every page (the old cursor is stale by contract, never reused).
  // Synchronized at the same places the observation snapshot is updated.
  function syncSections(next: ObservationAndScanSnapshot) {
    const identity = next.currentReport?.summary?.contentIdentity ?? null;
    if (identity !== lastReportIdentity.current) {
      lastReportIdentity.current = identity;
      setSections(EMPTY_SECTIONS);
      setSelected({});
      setWinners({});
    }
  }

  useEffect(() => {
    let current = true;
    let unlisten: (() => void) | null = null;
    client
      .getObservationSnapshot()
      .then((next) => {
        if (current) {
          setObservation(next);
          syncSections(next);
        }
      })
      .catch(() => undefined);
    client
      .listenObservationChanged((payload) => {
        if (current) {
          setObservation(payload);
          syncSections(payload);
        }
      })
      .then((stop) => {
        if (!current) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      current = false;
      unlisten?.();
    };
  }, [client]);

  const run = observation?.scanRun ?? null;
  const report = observation?.currentReport ?? null;
  const summary = report?.summary ?? null;
  const hasReport = summary !== null;
  const stale = report?.freshness === "stale";
  const cacheUnreadable =
    report?.staleReasons.includes("cache_unreadable") ?? false;

  // The summary sheet loads the first page of every candidate section and
  // the Root coverage table immediately when a Report arrives (spec §7.6:
  // the sheet presents coverage and candidates; "Load more" only pages on).
  const reportIdentity = summary?.contentIdentity ?? null;
  const reportIdentityRef = useRef<string | null>(null);
  useEffect(() => {
    if (!summary || reportIdentityRef.current === reportIdentity) return;
    reportIdentityRef.current = reportIdentity;
    for (const section of [
      "roots",
      "needs_attention",
      "git_sources",
      "local_candidates",
      "conflict_sets",
      "excluded",
    ] as const) {
      void loadSection(section, 0);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reportIdentity, summary]);

  async function start() {
    setBusy(true);
    setError(null);
    try {
      setObservation(await client.startRescan("manual"));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function cancel() {
    if (!run) return;
    setBusy(true);
    setError(null);
    try {
      setObservation(await client.cancelRescan(run.runId));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function loadSection(section: ScanReportSection, offset: number) {
    if (!summary) return;
    const current = sections[section];
    if (current.loading) return;
    setSections((prev) => ({
      ...prev,
      [section]: { ...prev[section], loading: true },
    }));
    try {
      const page = await client.getScanReportPage(
        {
          reportContentIdentity: summary.contentIdentity,
          runId: summary.runId,
          generation: summary.generation,
          section,
          offset,
        },
        PAGE_SIZE,
      );
      if (page.reportContentIdentity !== summary.contentIdentity) {
        setError(t("scan.ledger.pageStale"));
        setSections(EMPTY_SECTIONS);
        return;
      }
      setSections((prev) => {
        const existing = prev[section];
        const rows =
          offset === 0 ? page.rows : [...existing.rows, ...page.rows];
        return {
          ...prev,
          [section]: {
            rows,
            nextOffset: page.nextOffset,
            loading: false,
            loaded: true,
          },
        };
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setSections((prev) => ({
        ...prev,
        [section]: { ...prev[section], loading: false },
      }));
    }
  }

  function loadMore(section: ScanReportSection) {
    const current = sections[section];
    if (current.nextOffset === null && current.rows.length > 0) return;
    void loadSection(section, current.nextOffset ?? 0);
  }

  const runActive =
    run !== null && ["queued", "running", "cancelling"].includes(run.state);
  const terminalFailed = run?.state === "failed" || run?.state === "superseded";

  return (
    <section
      className="scan-evidence-ledger"
      aria-label={t("scan.ledger.label")}
      data-state={
        runActive ? "running" : (run?.state ?? (hasReport ? "report" : "none"))
      }
    >
      <div className="scan-ledger-status" role="status" aria-live="polite">
        <span className="scan-ledger-state">
          {runActive
            ? t(stateKey(run))
            : run
              ? t(stateKey(run))
              : t(reportStateKey(summary))}
        </span>
        {stale ? (
          <span className="scan-ledger-stale">{t("scan.ledger.stale")}</span>
        ) : null}
        {cacheUnreadable ? (
          <span className="scan-ledger-stale">
            {t("scan.ledger.cacheUnreadable")}
          </span>
        ) : null}
        {run ? (
          <span className="scan-ledger-phase">
            {t(phaseKey(run))} · {t(triggerKey(run))}
          </span>
        ) : null}
      </div>

      {run ? (
        <dl className="scan-ledger-facts">
          <dt>
            {t("scan.ledger.entryCount", { entries: run.counts.entries })}
          </dt>
          <dd>
            {t("scan.ledger.entityCount", { entities: run.counts.entities })}
          </dd>
          <dd>{t("scan.ledger.fileCount", { files: run.counts.files })}</dd>
          <dd>{t("scan.ledger.byteCount", { bytes: run.counts.bytes })}</dd>
          {run.counts.failedRoots > 0 ? (
            <dd>
              {t("scan.ledger.failedRootCount", {
                failed: run.counts.failedRoots,
                roots: run.counts.roots,
              })}
            </dd>
          ) : null}
          <dd>
            {t("scan.ledger.elapsed", {
              seconds: Math.floor(run.elapsedMs / 1000),
            })}
          </dd>
        </dl>
      ) : summary ? (
        <>
          {/* Funnel: Configured Agents → declared roots → canonical roots →
              appearances → canonical entities (spec §7.6). */}
          <ul className="scan-summary-funnel">
            <li>
              {t("scan.summary.funnel.agents", {
                value: summary.counts.configuredAgents,
              })}
            </li>
            <li>
              {t("scan.summary.funnel.declaredRoots", {
                value: summary.counts.declaredRoots,
              })}
            </li>
            <li>
              {t("scan.summary.funnel.canonicalRoots", {
                value: summary.counts.canonicalRoots,
              })}
            </li>
            <li>
              {t("scan.summary.funnel.appearances", {
                value: summary.counts.entries,
              })}
            </li>
            <li>
              {t("scan.summary.funnel.entities", {
                value: summary.counts.entities,
              })}
            </li>
          </ul>
          {/* Four classification count cards: counting only. */}
          <ul
            className="scan-summary-cards"
            aria-label={t("scan.summary.cards.label")}
          >
            {countCards(summary).map((card) => (
              <li key={card.key} aria-label={t(card.key)}>
                <span className="scan-summary-card-value">{card.value}</span>
                <span className="scan-summary-card-label">{t(card.key)}</span>
              </li>
            ))}
          </ul>
          {summary.incomplete ? (
            <p className="scan-ledger-slow" role="alert">
              {t("scan.summary.incompleteDestructiveDisabled")}
            </p>
          ) : null}
          <dl className="scan-ledger-facts">
            <dd>
              {t("scan.ledger.entryCount", {
                entries: summary.counts.entries,
              })}
            </dd>
            <dd>
              {t("scan.ledger.entityCount", {
                entities: summary.counts.entities,
              })}
            </dd>
            <dd>
              {t("scan.ledger.fileCount", {
                files: summary.counts.files,
              })}
            </dd>
            <dd>
              {t("scan.ledger.byteCount", { bytes: summary.counts.bytes })}
            </dd>
            <dd>
              {t("scan.ledger.published", {
                time: new Date(summary.publishedAtMs).toLocaleString(),
              })}
            </dd>
          </dl>
        </>
      ) : null}

      {run?.currentRoot !== null && run?.currentRoot !== undefined ? (
        <p className="scan-ledger-root">
          {t("scan.ledger.currentRoot", {
            index: run.currentRoot + 1,
            path: run.roots[run.currentRoot]?.configuredPath ?? "",
          })}
        </p>
      ) : null}
      {run?.slow ? (
        <p className="scan-ledger-slow">{t("scan.ledger.slow")}</p>
      ) : null}
      {run?.diagnostic ? (
        <p className="scan-ledger-diagnostic" role="alert">
          {run.diagnostic}
        </p>
      ) : null}
      {error ? (
        <p className="scan-ledger-error" role="alert">
          {error}
        </p>
      ) : null}

      {summary && !runActive ? (
        <div className="scan-summary-blocks">
          {/* §8.1 fixed block order. Incomplete first, only when published. */}
          {summary.incomplete ? (
            <section className="scan-summary-block scan-summary-block-incomplete">
              <h4>{t("scan.summary.block.incomplete")}</h4>
              <ul className="scan-summary-rows scan-summary-rows-incomplete">
                {sections.roots.rows
                  .filter(
                    (row) =>
                      row.kind === "root_coverage" && row.state !== "completed",
                  )
                  .map((row, index) => (
                    <li key={index} className="scan-summary-issue">
                      {row.kind === "root_coverage" ? (
                        <>
                          <span>{row.canonicalPath}</span>
                          <span className="scan-summary-issue-diagnostic">
                            {row.diagnostic ??
                              t(
                                `scan.ledger.rootState.${row.state}` as MessageKey,
                              )}
                          </span>
                        </>
                      ) : null}
                    </li>
                  ))}
                {sections.roots.loaded &&
                !sections.roots.rows.some(
                  (row) =>
                    row.kind === "root_coverage" && row.state !== "completed",
                ) ? (
                  <li>{t("scan.summary.noCandidates")}</li>
                ) : null}
              </ul>
            </section>
          ) : null}
          {CANDIDATE_SECTIONS.map((section) => (
            <section
              key={section}
              className="scan-summary-block"
              aria-label={t(sectionKey(section))}
            >
              <h4>{t(sectionKey(section))}</h4>
              <ul className="scan-summary-rows">
                {sections[section].rows.map((row, index) => {
                  const key = candidateKey(row);
                  const destination =
                    row.kind === "source_verdict" ? row.canonicalPath : "";
                  const operations = operationEligibility(row);
                  const blocked = operations.filter((op) => !op.allowed);
                  const clearable = operations.length > 0;
                  return (
                    <li
                      key={index}
                      className={`scan-summary-row scan-summary-row-${row.kind}`}
                    >
                      <div className="scan-summary-row-destination">
                        <span className="scan-summary-row-text">
                          {rowText(row, t)}
                        </span>
                        <span className="scan-summary-row-path">
                          {destination}
                        </span>
                        {row.kind === "source_verdict" &&
                        row.verdict !== "local" ? (
                          <span className="scan-summary-row-reason">
                            {reasonKey(row.reasonKind)
                              ? t(reasonKey(row.reasonKind) as MessageKey)
                              : t(verdictKey(row.verdict))}
                          </span>
                        ) : row.kind === "git_source_group" &&
                          row.status !== "candidate" ? (
                          <span className="scan-summary-row-reason">
                            {t(statusKey(row.status))}
                          </span>
                        ) : null}
                        {row.kind === "source_verdict"
                          ? row.notes.map((note) => (
                              <span
                                key={note}
                                className="scan-summary-row-note"
                              >
                                {t(noteKey(note))}
                              </span>
                            ))
                          : null}
                      </div>
                      {/* Typed operation eligibility: default-empty selection
                          draft; destructive ops blocked with the Core closed
                          reason; Blocked/Deferred have no control at all. */}
                      {clearable && !idle ? (
                        <div className="scan-summary-row-operations">
                          {operations.map((op) =>
                            op.allowed ? (
                              <label
                                key={op.operation}
                                className="scan-summary-row-op"
                              >
                                <input
                                  type="checkbox"
                                  checked={selected[key] ?? false}
                                  onChange={(event) =>
                                    setSelected((prev) => ({
                                      ...prev,
                                      [key]: event.target.checked,
                                    }))
                                  }
                                />
                                {t(operationKey(op.operation))}
                              </label>
                            ) : (
                              <span
                                key={op.operation}
                                className="scan-summary-row-op-blocked"
                                aria-disabled={true}
                                title={t(
                                  "scan.summary.incompleteDestructiveDisabled",
                                )}
                              >
                                {t(operationKey(op.operation))}
                                <span className="scan-summary-row-op-blocked-hint">
                                  {t("scan.summary.op.blocked")}
                                </span>
                              </span>
                            ),
                          )}
                        </div>
                      ) : null}
                      {blocked.length > 0 ? (
                        <p className="scan-summary-row-blocked-reason">
                          {t("scan.summary.incompleteDestructiveDisabled")}
                        </p>
                      ) : null}
                    </li>
                  );
                })}
                {sections[section].loaded &&
                sections[section].rows.length === 0 ? (
                  <li className="scan-summary-none">
                    {t("scan.summary.noCandidates")}
                  </li>
                ) : null}
              </ul>
              {!sections[section].loaded ||
              sections[section].nextOffset !== null ? (
                <button
                  type="button"
                  onClick={() => loadMore(section)}
                  disabled={sections[section].loading}
                >
                  {t("scan.ledger.loadMore")}
                </button>
              ) : null}
              {/* §7.6: Conflict Sets (Local↔Local same Directory Identity,
                  default no winner) render inside the Local sources block. */}
              {section === "local_candidates" ? (
                <div className="scan-summary-conflicts">
                  <h5>{t("scan.summary.block.conflictSets")}</h5>
                  <ul className="scan-summary-rows">
                    {sections.conflict_sets.rows.map((row, index) => {
                      if (row.kind !== "conflict_set") return null;
                      const setKey = candidateKey(row);
                      return (
                        <li
                          key={index}
                          className="scan-summary-row scan-summary-row-conflict_set"
                        >
                          <div className="scan-summary-row-destination">
                            <span className="scan-summary-row-text">
                              {rowText(row, t)}
                            </span>
                            <span className="scan-summary-row-path">
                              {t("scan.summary.conflict.noWinnerYet")}
                            </span>
                          </div>
                          {!idle ? (
                            <div className="scan-summary-row-operations">
                              {row.memberEntitySeqs.map((entitySeq) => (
                                <label
                                  key={entitySeq}
                                  className="scan-summary-row-op"
                                >
                                  <input
                                    type="radio"
                                    name={`winner-${row.setSeq}`}
                                    checked={
                                      (winners[setKey] ?? -1) === entitySeq
                                    }
                                    onChange={() =>
                                      setWinners((prev) => ({
                                        ...prev,
                                        [setKey]: entitySeq,
                                      }))
                                    }
                                  />
                                  {t("scan.summary.conflict.winner", {
                                    member: entitySeq,
                                  })}
                                </label>
                              ))}
                            </div>
                          ) : null}
                        </li>
                      );
                    })}
                  </ul>
                  {!sections.conflict_sets.loaded ||
                  sections.conflict_sets.nextOffset !== null ? (
                    <button
                      type="button"
                      onClick={() =>
                        loadSection(
                          "conflict_sets",
                          sections.conflict_sets.nextOffset ?? 0,
                        )
                      }
                      disabled={sections.conflict_sets.loading}
                    >
                      {t("scan.ledger.loadMore")}
                    </button>
                  ) : null}
                </div>
              ) : null}
            </section>
          ))}
        </div>
      ) : null}

      {summary && !runActive ? (
        <div className="scan-ledger-sections">
          {(["roots", "entities", "appearances", "diagnostics"] as const).map(
            (section) => {
              const state = sections[section];
              const hasMore = !state.loaded || state.nextOffset !== null;
              return (
                <details key={section} className={`scan-ledger-section`}>
                  <summary>{t(sectionKey(section))}</summary>
                  {section === "roots" ? (
                    <table className="scan-summary-coverage">
                      <thead>
                        <tr>
                          <th>{t("scan.summary.coverage.root")}</th>
                          <th>{t("scan.summary.coverage.agents")}</th>
                          <th>{t("scan.summary.coverage.result")}</th>
                          <th>{t("scan.summary.coverage.evidence")}</th>
                        </tr>
                      </thead>
                      <tbody>
                        {state.rows.map((row, index) =>
                          row.kind === "root_coverage" ? (
                            <tr key={index}>
                              <td>{row.canonicalPath}</td>
                              <td>
                                {row.consumerAgents.length > 0
                                  ? row.consumerAgents
                                      .map((agent) => agent.agentName)
                                      .join(", ")
                                  : t("scan.summary.coverage.noAgents")}
                              </td>
                              <td>
                                {t(
                                  `scan.ledger.rootState.${row.state}` as MessageKey,
                                )}
                                {row.diagnostic ? (
                                  <span
                                    className="scan-summary-coverage-diagnostic"
                                    role="alert"
                                  >
                                    {row.diagnostic}
                                  </span>
                                ) : null}
                              </td>
                              <td>
                                {t("scan.summary.coverage.evidenceSummary", {
                                  entities: row.counts.entities,
                                  files: row.counts.files,
                                  entries: row.counts.entries,
                                  bytes: row.counts.bytes,
                                })}
                              </td>
                            </tr>
                          ) : null,
                        )}
                        {state.loaded && state.rows.length === 0 ? (
                          <tr>
                            <td colSpan={4}>{t("scan.ledger.noRows")}</td>
                          </tr>
                        ) : null}
                      </tbody>
                    </table>
                  ) : (
                    <>
                      {state.loaded && state.rows.length === 0 ? (
                        <p className="scan-ledger-none">
                          {t("scan.ledger.noRows")}
                        </p>
                      ) : (
                        <ul className="scan-ledger-rows">
                          {state.rows.map((row, index) => (
                            <li key={index}>{rowText(row, t)}</li>
                          ))}
                        </ul>
                      )}
                    </>
                  )}
                  {hasMore ? (
                    <button
                      type="button"
                      onClick={() => loadMore(section)}
                      disabled={state.loading}
                    >
                      {t("scan.ledger.loadMore")}
                    </button>
                  ) : null}
                </details>
              );
            },
          )}
        </div>
      ) : null}

      {!idle ? (
        <div className="scan-ledger-actions">
          {runActive && run.state !== "cancelling" ? (
            <button type="button" onClick={cancel} disabled={busy}>
              {t("scan.ledger.cancel")}
            </button>
          ) : null}
          {!runActive || terminalFailed ? (
            <button type="button" onClick={start} disabled={busy}>
              {t("scan.ledger.rescan")}
            </button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
