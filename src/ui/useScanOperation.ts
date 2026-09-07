import { useEffect, useRef } from "react";
import type { ObservationAndScanSnapshot } from "../app/catalog-client";
import { useLocale } from "../features/locale/LocaleProvider";
import { useBackgroundOperations } from "./BackgroundOperations";

/** Only runs observed in progress can produce a completion, never an old report. */
export function useScanOperation(snapshot: ObservationAndScanSnapshot | null) {
  const operations = useBackgroundOperations();
  const { t } = useLocale();
  const active = useRef<{ runId: string; noticeId: string } | null>(null);
  useEffect(() => {
    const run = snapshot?.scanRun;
    if (!run || run.trigger === "onboarding") return;
    if (["queued", "running", "cancelling"].includes(run.state)) {
      const notice = {
        title: t(
          run.state === "cancelling"
            ? "scan.ledger.cancelling"
            : "operation.background.scan",
        ),
        detail:
          t(`scan.ledger.phase.${run.phase}`) +
          " · " +
          t("operation.background.working"),
      };
      if (active.current?.runId !== run.runId) {
        if (active.current)
          operations.finish(active.current.noticeId, {
            title: t("operation.background.scan_cancelled"),
            detail: t("operation.background.scan_retry"),
            state: "cancelled",
          });
        active.current = {
          runId: run.runId,
          noticeId: operations.begin(notice),
        };
      } else operations.update(active.current.noticeId, notice);
      return;
    }
    if (active.current?.runId !== run.runId) return;
    const summary = snapshot.currentReport.summary;
    if (run.state === "completed" && summary?.runId !== run.runId) return;
    const completed =
      run.state === "completed" &&
      summary?.state === "complete" &&
      !summary.incomplete;
    const cancelled = run.state === "cancelled" || run.state === "superseded";
    operations.finish(active.current.noticeId, {
      title: t(
        completed
          ? "operation.background.scan_done"
          : cancelled
            ? "operation.background.scan_cancelled"
            : "operation.background.scan_failed",
      ),
      detail: t(
        completed
          ? "operation.background.scan_next"
          : "operation.background.scan_retry",
      ),
      state: completed ? "completed" : cancelled ? "cancelled" : "failed",
    });
    active.current = null;
  }, [snapshot, operations, t]);
}
