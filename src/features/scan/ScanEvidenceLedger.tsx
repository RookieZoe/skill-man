import { useEffect, useState } from "react";

import type {
  CatalogClient,
  CurrentReport,
  ObservationAndScanSnapshot,
  ScanRunSnapshot,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import type { MessageKey } from "../locale/messages";

/**
 * Non-modal Evidence Ledger for the full Rescan (spec §4.10, ADR-0020): the
 * shared state surface on Library Desk and Agent Management. It only shows
 * honest facts — state, phase, root, counts, elapsed, Slow — never a percent
 * or an ETA (the total scale is unknown). The report stays on screen while a
 * Run is active; Reports loaded from a previous launch are marked Stale.
 *
 * Full Root/entity/appearance/diagnostic pagination arrives with #83's
 * `report_page` contract; this surface is the bounded summary.
 */

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

  useEffect(() => {
    let current = true;
    let unlisten: (() => void) | null = null;
    client
      .getObservationSnapshot()
      .then((next) => {
        if (current) setObservation(next);
      })
      .catch(() => undefined);
    client
      .listenObservationChanged((payload) => {
        if (current) setObservation(payload);
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
  const hasReport = (report?.summary ?? null) !== null;
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
              : t(reportStateKey(report?.summary ?? null))}
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
      ) : report?.summary ? (
        <dl className="scan-ledger-facts">
          <dd>
            {t("scan.ledger.entryCount", {
              entries: report.summary.counts.entries,
            })}
          </dd>
          <dd>
            {t("scan.ledger.entityCount", {
              entities: report.summary.counts.entities,
            })}
          </dd>
          <dd>
            {t("scan.ledger.fileCount", { files: report.summary.counts.files })}
          </dd>
          <dd>
            {t("scan.ledger.byteCount", { bytes: report.summary.counts.bytes })}
          </dd>
        </dl>
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
