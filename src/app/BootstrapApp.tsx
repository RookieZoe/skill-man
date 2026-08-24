import { useCallback, useEffect, useState } from "react";

import type {
  BootstrapDiagnostic,
  BootstrapSnapshot,
  CatalogClient,
} from "./catalog-client";
import { App } from "./App";
import { RecoveryView } from "../features/recovery/RecoveryView";
import { RestoreView } from "../features/recovery/RestoreView";
import {
  HomeBindingView,
  type HomePicker,
} from "../features/home/HomeBindingView";
import { LifecycleRoute } from "../features/home/LifecycleRoute";
import { DefaultHomeRecoveryView } from "../features/home/DefaultHomeRecoveryView";
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
      // A same-identity content failure (integrity/foreign-key) is the
      // Restore route: browsing a corrupt Catalog is never offered, the
      // whole Home restores with the same home_id (ADR-0012 §6).
      return snapshot.catalogReadonlyReason === "integrity_failed" ? (
        <RestoreView client={client} onSnapshot={onSnapshot} />
      ) : (
        <App client={client} />
      );
    case "unconfigured":
    case "abandoned":
      return (
        <HomeBindingView
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          pickDirectory={pickDirectory}
        />
      );
    case "default_home_recovery_offer":
      return (
        <DefaultHomeRecoveryView
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
        />
      );
    case "default_home_recovery_blocked":
      return (
        <BootstrapRoute
          title={t("bootstrap.default_recovery.blocked_title")}
          summary={t("bootstrap.default_recovery.blocked_summary", {
            path: snapshot.path,
          })}
          onRetry={onRetry}
          diagnostic={{
            code: "default_home_recovery_blocked",
            message: snapshot.reason,
          }}
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
        <LifecycleRoute
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          onRetry={onRetry}
        />
      );
    case "home_identity_mismatch":
      return (
        <LifecycleRoute
          client={client}
          snapshot={snapshot}
          onSnapshot={onSnapshot}
          onRetry={onRetry}
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
