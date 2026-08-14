import { useCallback, useEffect, useState } from "react";

import type {
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
  RestoreEligibility,
  SafetySnapshot,
} from "../../app/catalog-client";
import { LanguageControl } from "../locale/LanguageControl";
import { useLocale } from "../locale/LocaleProvider";
import { errorMessageKey, errorMessageParams } from "../locale/messages";
import { SafetySnapshots } from "./SafetySnapshots";

export interface RestoreViewProps {
  client: CatalogClient;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
  /** Back to the probing route (HomeUnavailable); hidden on the Bound route. */
  onBack?: () => void;
}

type BusyAction = "probe" | "start" | "continue" | "commit" | "delete" | null;

/**
 * Restore Bound Home (spec §5.5, ADR-0012 §6): the same crash-convergent
 * recovery state machine as Fixture Recovery, entered from a same-identity
 * content failure. The locator identity is never modified; the Safety
 * Snapshot is never auto-deleted; Activations are never touched.
 */
export function RestoreView({ client, onSnapshot, onBack }: RestoreViewProps) {
  const { t } = useLocale();
  const [eligibility, setEligibility] = useState<RestoreEligibility | null>(
    null,
  );
  const [probeFailed, setProbeFailed] = useState(false);
  const [operationId, setOperationId] = useState<string | null>(null);
  const [phase, setPhase] = useState<"idle" | "prepared">("idle");
  const [snapshots, setSnapshots] = useState<SafetySnapshot[]>([]);
  const [error, setError] = useState<CommandFailure | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<BusyAction>(null);

  const refresh = useCallback(() => {
    client
      .listSafetySnapshots()
      .then(setSnapshots)
      .catch(() => setSnapshots([]));
  }, [client]);

  const probe = useCallback(() => {
    client
      .getRestoreEligibility()
      .then((next) => {
        setEligibility(next);
        setProbeFailed(false);
      })
      .catch(() => setProbeFailed(true));
  }, [client]);

  useEffect(() => {
    probe();
    refresh();
  }, [probe, refresh]);

  const run = useCallback(
    async (action: BusyAction, operation: () => Promise<unknown>) => {
      setBusy(action);
      setError(null);
      setNotice(null);
      try {
        await operation();
        refresh();
      } catch (failure) {
        setError(failure as CommandFailure);
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  if (probeFailed) {
    return (
      <section className="recovery-route" aria-labelledby="restore-title">
        <LanguageControl />
        <h1 id="restore-title">{t("restore.title")}</h1>
        <p>{t("restore.command_failed")}</p>
        <button type="button" onClick={refresh}>
          {t("recovery.retry")}
        </button>
        {onBack ? (
          <button type="button" onClick={onBack}>
            {t("restore.action.back")}
          </button>
        ) : null}
      </section>
    );
  }
  if (!eligibility) {
    return (
      <section className="recovery-route" aria-labelledby="restore-title">
        <LanguageControl />
        <h1 id="restore-title">{t("restore.title")}</h1>
        <p>{t("recovery.resolving")}</p>
      </section>
    );
  }

  const eligible = eligibility.kind === "restore_required";
  const reason =
    eligibility.kind === "restore_required" ? eligibility.reason : null;
  const awaitingCommit = phase === "prepared";
  const canContinue = operationId !== null && !awaitingCommit;

  return (
    <section className="recovery-route" aria-labelledby="restore-title">
      <LanguageControl />
      <h1 id="restore-title">{t("restore.title")}</h1>
      {eligible ? (
        <>
          <p className="recovery-summary">
            {t("restore.summary", { path: eligibility.path })}
          </p>
          <p className="recovery-path">{eligibility.homeId}</p>
          <p className="restore-reason">
            {reason === "catalog_integrity_failed"
              ? t("restore.reason.integrity")
              : t("restore.reason.fixture")}
          </p>
        </>
      ) : (
        <p className="recovery-summary">
          {eligibility.kind === "not_required"
            ? t("restore.not_applicable.healthy")
            : t(`restore.not_applicable.${eligibility.reason}`)}
        </p>
      )}

      {error ? (
        <div className="recovery-notice recovery-notice--error" role="alert">
          <p>
            {t(
              errorMessageKey(error.error.code),
              errorMessageParams(error.error),
            )}
          </p>
          {error.diagnostic ? (
            <details className="bootstrap-diagnostic">
              <summary>{t("restore.technical_details")}</summary>
              <dl>
                <dt>{t("bootstrap.code")}</dt>
                <dd>{error.diagnostic.code}</dd>
                <dt>{t("bootstrap.detail")}</dt>
                <dd>{error.diagnostic.message}</dd>
              </dl>
            </details>
          ) : null}
        </div>
      ) : null}
      {notice ? (
        <div className="recovery-notice" role="status">
          <p>{notice}</p>
        </div>
      ) : null}

      <div className="recovery-actions" aria-live="polite">
        {eligible && operationId === null ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("start", async () => {
                const plan = await client.planRestore();
                const result = await client.applyFixtureRecovery(
                  plan.planToken,
                );
                if (!result.awaitingCommit) {
                  // The state machine only returns awaiting_commit or a
                  // rollback; anything else fails closed.
                  setNotice(t("restore.step.rolled_back"));
                  probe();
                  return;
                }
                setOperationId(result.operationId);
                setPhase("prepared");
                setNotice(t("restore.step.prepared"));
              })
            }
          >
            {busy === "start"
              ? t("restore.action.start_busy")
              : t("restore.action.start")}
          </button>
        ) : null}
        {canContinue && operationId ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("continue", async () => {
                const result = await client.applyFixtureRecovery(operationId);
                if (result.rolledBack) {
                  setNotice(t("restore.step.rolled_back"));
                  setOperationId(null);
                  setPhase("idle");
                  probe();
                } else {
                  setPhase("prepared");
                  setNotice(t("restore.step.prepared"));
                }
              })
            }
          >
            {busy === "continue"
              ? t("restore.action.continuing")
              : t("restore.action.continue")}
          </button>
        ) : null}
        {awaitingCommit && operationId ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() =>
              void run("commit", async () => {
                const snapshot =
                  await client.confirmFixtureRecoveryResult(operationId);
                onSnapshot(snapshot);
              })
            }
          >
            {busy === "commit"
              ? t("restore.action.committing")
              : t("restore.action.commit")}
          </button>
        ) : null}
        {onBack ? (
          <button type="button" disabled={busy !== null} onClick={onBack}>
            {t("restore.action.back")}
          </button>
        ) : null}
      </div>

      <SafetySnapshots
        client={client}
        snapshots={snapshots}
        busy={busy !== null}
        run={(operation) => run("delete", operation)}
      />
    </section>
  );
}
