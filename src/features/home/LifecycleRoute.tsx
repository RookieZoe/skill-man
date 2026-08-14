import { useState } from "react";

import type {
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { LanguageControl } from "../locale/LanguageControl";
import { errorMessageKey, errorMessageParams } from "../locale/messages";
import { AbandonFlow } from "./AbandonFlow";
import { RestoreView } from "../recovery/RestoreView";

interface LifecycleRouteProps {
  client: CatalogClient;
  snapshot:
    | Extract<BootstrapSnapshot, { state: "home_unavailable" }>
    | Extract<BootstrapSnapshot, { state: "home_identity_mismatch" }>;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
  onRetry: () => void;
}

type BusyAction = "reconnect" | "probe" | null;

/**
 * The fail-closed HomeUnavailable / HomeIdentityMismatch route (spec §5.5,
 * ADR-0012 §5): only the legal actions of the current state are exposed —
 * Retry, Reconnect Same Home, the Restore eligibility probe (unavailable
 * only) and the high-friction Abandon. Locale stays independent of the
 * Home and remains writable.
 */
export function LifecycleRoute({
  client,
  snapshot,
  onSnapshot,
  onRetry,
}: LifecycleRouteProps) {
  const { t } = useLocale();
  const isMismatch = snapshot.state === "home_identity_mismatch";
  const [busy, setBusy] = useState<BusyAction>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<CommandFailure | null>(null);
  const [abandonOpen, setAbandonOpen] = useState(false);
  const [restoreOpen, setRestoreOpen] = useState(false);

  const reconnect = async () => {
    setBusy("reconnect");
    setError(null);
    setNotice(null);
    try {
      const next = await client.reconnectSameHome();
      onSnapshot(next);
      setNotice(
        next.state === "bound"
          ? t("lifecycle.reconnect_success")
          : t("lifecycle.reconnect_failed"),
      );
    } catch (failure) {
      setError(failure as CommandFailure);
    } finally {
      setBusy(null);
    }
  };

  const probeRestore = async () => {
    setBusy("probe");
    setError(null);
    setNotice(null);
    try {
      const eligibility = await client.getRestoreEligibility();
      if (eligibility.kind === "restore_required") {
        setRestoreOpen(true);
      } else {
        setNotice(
          eligibility.kind === "not_required"
            ? t("restore.not_applicable.healthy")
            : t(`restore.not_applicable.${eligibility.reason}`),
        );
      }
    } catch (failure) {
      setError(failure as CommandFailure);
    } finally {
      setBusy(null);
    }
  };

  const title = isMismatch
    ? t("bootstrap.route.mismatch_title")
    : t("bootstrap.route.unavailable_title");
  const summary = isMismatch
    ? t("bootstrap.route.mismatch_summary", { path: snapshot.path })
    : t("bootstrap.route.unavailable_summary", { path: snapshot.path });

  if (restoreOpen) {
    return (
      <RestoreView
        client={client}
        onSnapshot={onSnapshot}
        onBack={() => setRestoreOpen(false)}
      />
    );
  }

  return (
    <main
      role="status"
      aria-live="polite"
      className="bootstrap-route"
      data-bootstrap-route
    >
      <h1>{title}</h1>
      <p>{summary}</p>
      <div className="bootstrap-actions" aria-live="polite">
        <button type="button" disabled={busy !== null} onClick={onRetry}>
          {t("bootstrap.retry")}
        </button>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => void reconnect()}
        >
          {busy === "reconnect"
            ? t("lifecycle.reconnect_busy")
            : t("lifecycle.reconnect")}
        </button>
        {!isMismatch ? (
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => void probeRestore()}
          >
            {busy === "probe"
              ? t("recovery.resolving")
              : t("lifecycle.restore")}
          </button>
        ) : null}
        <button
          type="button"
          disabled={busy !== null}
          onClick={() => setAbandonOpen(true)}
        >
          {t("lifecycle.abandon")}
        </button>
      </div>
      {error ? (
        <div className="bootstrap-error" role="alert">
          <p>
            {t(
              errorMessageKey(error.error.code),
              errorMessageParams(error.error),
            )}
          </p>
          {error.diagnostic ? (
            <details className="bootstrap-diagnostic">
              <summary>{t("bootstrap.technical_details")}</summary>
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
        <p className="bootstrap-notice" role="status">
          {notice}
        </p>
      ) : null}
      {snapshot.diagnostic ? (
        <details className="bootstrap-diagnostic">
          <summary>{t("bootstrap.technical_details")}</summary>
          <dl>
            <dt>{t("bootstrap.code")}</dt>
            <dd>{snapshot.diagnostic.code}</dd>
            <dt>{t("bootstrap.detail")}</dt>
            <dd>{snapshot.diagnostic.message}</dd>
          </dl>
        </details>
      ) : null}
      <LanguageControl />
      {abandonOpen ? (
        <AbandonFlow
          client={client}
          onSnapshot={onSnapshot}
          onClose={() => setAbandonOpen(false)}
        />
      ) : null}
    </main>
  );
}
