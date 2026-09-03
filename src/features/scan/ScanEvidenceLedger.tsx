import { useEffect, useRef, useState } from "react";

import type {
  CatalogClient,
  CurrentReport,
  ObservationAndScanSnapshot,
  ScanReportRow,
  ScanReportSection,
  ScanRunSnapshot,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import type { MessageKey, MessageParams } from "../locale/messages";

/**
 * Non-modal Evidence Ledger for the full Rescan (spec §4.10, ADR-0020): the
 * shared state surface on Library Desk and Agent Management. It only shows
 * honest facts — state, phase, root, counts, elapsed, Slow — never a percent
 * or an ETA (the total scale is unknown). The report stays on screen while a
 * Run is active; Reports loaded from a previous launch are marked Stale.
 *
 * Full Root coverage / entity / appearance / diagnostic detail is served by
 * the unique generation-bound `getScanReportPage` contract (spec §4.10;
 * ADR-0017): every section keeps a stable cursor and bounded pages, and a
 * Report switch makes an old cursor typed stale instead of falling back.
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
};

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
  }
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
  }
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

  // Page sections are bound to one Report identity: a published new Report
  // resets every page (the old cursor is stale by contract, never reused).
  // Synchronized at the same places the observation snapshot is updated.
  function syncSections(next: ObservationAndScanSnapshot) {
    const identity = next.currentReport?.summary?.contentIdentity ?? null;
    if (identity !== lastReportIdentity.current) {
      lastReportIdentity.current = identity;
      setSections(EMPTY_SECTIONS);
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

  async function loadMore(section: ScanReportSection) {
    if (!summary) return;
    const current = sections[section];
    if (current.loading) return;
    if (current.nextOffset === null && current.rows.length > 0) return;
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
          offset: current.nextOffset ?? 0,
        },
        PAGE_SIZE,
      );
      // The page is accepted only when the Report identity still matches.
      if (page.reportContentIdentity !== summary.contentIdentity) {
        setError(t("scan.ledger.pageStale"));
        setSections(EMPTY_SECTIONS);
        return;
      }
      setSections((prev) => ({
        ...prev,
        [section]: {
          rows: [...prev[section].rows, ...page.rows],
          nextOffset: page.nextOffset,
          loading: false,
          loaded: true,
        },
      }));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setSections((prev) => ({
        ...prev,
        [section]: { ...prev[section], loading: false },
      }));
    }
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
          <p className="scan-ledger-funnel">
            {t("scan.ledger.funnel", {
              agents: summary.counts.configuredAgents,
              declared: summary.counts.declaredRoots,
              canonical: summary.counts.canonicalRoots,
              appearances: summary.counts.entries,
              entities: summary.counts.entities,
            })}
          </p>
          <p className="scan-ledger-coverage">
            {t("scan.ledger.coverage", {
              completed: summary.coverage.completed,
              failed: summary.coverage.failed,
              unresponsive: summary.coverage.unresponsive,
            })}
          </p>
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
          {summary.incomplete ? (
            <p className="scan-ledger-slow" role="alert">
              {t("scan.ledger.incompleteCoverage")}
            </p>
          ) : null}
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
        <div className="scan-ledger-sections">
          {(["roots", "entities", "appearances", "diagnostics"] as const).map(
            (section) => {
              const state = sections[section];
              const hasMore = !state.loaded || state.nextOffset !== null;
              return (
                <details key={section} className={`scan-ledger-section`}>
                  <summary>{t(sectionKey(section))}</summary>
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
