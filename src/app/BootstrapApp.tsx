import { useCallback, useEffect, useState } from "react";

import type {
  BootstrapDiagnostic,
  BootstrapSnapshot,
  CatalogClient,
} from "./catalog-client";
import { App } from "./App";
import { RecoveryView } from "../features/recovery/RecoveryView";
import { HomeBindingView, type HomePicker } from "../features/home/HomeBindingView";
import { LocaleProvider } from "../features/locale/LocaleProvider";
import { LanguageControl } from "../features/locale/LanguageControl";
import { useLocale } from "../features/locale/LocaleProvider";

export interface BootstrapAppProps {
  client: CatalogClient;
  pickDirectory?: HomePicker;
}

/**
 * The unique top-level bootstrap route (spec §4.2, §5.1): the native
 * snapshot decides which single route renders. `Bound` renders the Library
 * Desk; every closed variant renders its own accessible route — the app never
 * guesses state or falls back to fixture data.
 */
export function BootstrapApp({ client, pickDirectory }: BootstrapAppProps) {
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);

  const refresh = useCallback(() => {
    client
      .getBootstrapSnapshot()
      .then((next) => {
        setSnapshot(next);
        setLoadFailed(false);
      })
      .catch(() => setLoadFailed(true));
  }, [client]);

  useEffect(() => {
    refresh();
    let unlisten: (() => void) | null = null;
    client
      .listenBootstrapChanged((payload) => {
        setSnapshot(payload.snapshot);
      })
      .then((stop) => {
        unlisten = stop;
      });
    return () => {
      unlisten?.();
    };
  }, [client, refresh]);

  return (
    <LocaleProvider client={client}>
      <BootstrapRoutes
        client={client}
        snapshot={snapshot}
        loadFailed={loadFailed}
        onRetry={refresh}
        onSnapshot={setSnapshot}
        pickDirectory={pickDirectory}
      />
    </LocaleProvider>
  );
}

function BootstrapRoutes({
  client,
  snapshot,
  loadFailed,
  onRetry,
  onSnapshot,
  pickDirectory,
}: {
  client: CatalogClient;
  snapshot: BootstrapSnapshot | null;
  loadFailed: boolean;
  onRetry: () => void;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
  pickDirectory?: HomePicker;
}) {
  const { t } = useLocale();

  if (loadFailed) {
    return (
      <BootstrapRoute
        title={t("bootstrap.failed_title")}
        summary={t("bootstrap.failed_summary")}
        onRetry={onRetry}
        diagnostic={{
          code: "bootstrap_command_failed",
          message: t("bootstrap.diagnostic_message"),
        }}
      />
    );
  }
  if (!snapshot) {
    return (
      <BootstrapRoute
        title={t("bootstrap.brand")}
        summary={t("bootstrap.resolving")}
      />
    );
  }
  switch (snapshot.state) {
    case "bound":
      return <App client={client} />;
    case "unconfigured":
      return (
        <HomeBindingView
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          pickDirectory={pickDirectory}
        />
      );
    case "legacy_detected":
      return (
        <HomeBindingView
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          pickDirectory={pickDirectory}
        />
      );
    case "fixture_recovery_locked":
      return <RecoveryView client={client} />;
    case "home_candidate_pending":
      return (
        <HomeBindingView
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          pickDirectory={pickDirectory}
        />
      );
    case "home_unavailable":
      return (
        <BootstrapRoute
          title={t("bootstrap.route.unavailable_title")}
          summary={t("bootstrap.route.unavailable_summary", {
            path: snapshot.path,
          })}
          diagnostic={snapshot.diagnostic ?? undefined}
        />
      );
    case "home_identity_mismatch":
      return (
        <BootstrapRoute
          title={t("bootstrap.route.mismatch_title")}
          summary={t("bootstrap.route.mismatch_summary", {
            path: snapshot.path,
          })}
          diagnostic={snapshot.diagnostic ?? undefined}
        />
      );
    case "app_state_unavailable":
      return (
        <BootstrapRoute
          title={t("bootstrap.route.state_title")}
          summary={t("bootstrap.route.state_summary")}
          onRetry={onRetry}
          diagnostic={snapshot.diagnostic ?? undefined}
        />
      );
  }
}

interface BootstrapRouteProps {
  title: string;
  summary: string;
  onRetry?: () => void;
  diagnostic?: BootstrapDiagnostic;
}

/** Closed-state shell: real accessible DOM, never prototype labels. */
function BootstrapRoute({
  title,
  summary,
  onRetry,
  diagnostic,
}: BootstrapRouteProps) {
  const { t } = useLocale();
  return (
    <main
      role="status"
      aria-live="polite"
      className="bootstrap-route"
      data-bootstrap-route
    >
      <h1>{title}</h1>
      <p>{summary}</p>
      <LanguageControl />
      {onRetry ? (
        <button type="button" onClick={onRetry}>
          {t("bootstrap.retry")}
        </button>
      ) : null}
      {diagnostic ? (
        <details className="bootstrap-diagnostic">
          <summary>{t("bootstrap.technical_details")}</summary>
          <dl>
            <dt>{t("bootstrap.code")}</dt>
            <dd>{diagnostic.code}</dd>
            <dt>{t("bootstrap.detail")}</dt>
            <dd>{diagnostic.message}</dd>
          </dl>
        </details>
      ) : null}
    </main>
  );
}
