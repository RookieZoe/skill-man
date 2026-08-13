import { useCallback, useEffect, useState } from "react";

import type {
  BootstrapDiagnostic,
  BootstrapSnapshot,
  CatalogClient,
} from "./catalog-client";
import { App } from "./App";
import { RecoveryView } from "../features/recovery/RecoveryView";

export interface BootstrapAppProps {
  client: CatalogClient;
}

/**
 * The unique top-level bootstrap route (spec §4.2, §5.1): the native
 * snapshot decides which single route renders. `Bound` renders the Library
 * Desk; every closed variant renders its own accessible route — the app never
 * guesses state or falls back to fixture data.
 */
export function BootstrapApp({ client }: BootstrapAppProps) {
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

  if (loadFailed) {
    return (
      <BootstrapRoute
        title="Skill Man could not start"
        summary="The bootstrap state could not be read. Retry to resolve it again."
        onRetry={refresh}
        diagnostic={{
          code: "bootstrap_command_failed",
          message: "The bootstrap snapshot command failed.",
        }}
      />
    );
  }
  if (!snapshot) {
    return (
      <BootstrapRoute
        title="Skill Man"
        summary="Resolving the Skill Man Home…"
      />
    );
  }
  switch (snapshot.state) {
    case "bound":
      return <App client={client} />;
    case "unconfigured":
      return (
        <BootstrapRoute
          title="Unconfigured"
          summary="No Skill Man Home is bound yet. The binding setup completes here in the Home Binding flow."
        />
      );
    case "legacy_detected":
      return (
        <BootstrapRoute
          title="Legacy Home detected"
          summary={`A Legacy Home exists at ${snapshot.path}. It is classified read-only before the one-time transition.`}
        />
      );
    case "fixture_recovery_locked":
      return <RecoveryView client={client} />;
    case "home_candidate_pending":
      return (
        <BootstrapRoute
          title="Home candidate pending"
          summary={`The candidate at ${snapshot.path} is awaiting its final confirmation.`}
        />
      );
    case "home_unavailable":
      return (
        <BootstrapRoute
          title="Home Unavailable"
          summary={`The bound Home at ${snapshot.path} cannot be reached. Nothing is written or re-bound.`}
          diagnostic={snapshot.diagnostic ?? undefined}
        />
      );
    case "home_identity_mismatch":
      return (
        <BootstrapRoute
          title="Home Identity Mismatch"
          summary={`The path ${snapshot.path} is not the bound Home. Its content is never treated as Bound.`}
          diagnostic={snapshot.diagnostic ?? undefined}
        />
      );
    case "app_state_unavailable":
      return (
        <BootstrapRoute
          title="App State Unavailable"
          summary="The app state could not be read or is contradictory. Retry to resolve it again."
          onRetry={refresh}
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
  return (
    <main
      role="status"
      aria-live="polite"
      className="bootstrap-route"
      data-bootstrap-route
    >
      <h1>{title}</h1>
      <p>{summary}</p>
      {onRetry ? (
        <button type="button" onClick={onRetry}>
          Retry
        </button>
      ) : null}
      {diagnostic ? (
        <details className="bootstrap-diagnostic">
          <summary>Technical details</summary>
          <dl>
            <dt>Code</dt>
            <dd>{diagnostic.code}</dd>
            <dt>Detail</dt>
            <dd>{diagnostic.message}</dd>
          </dl>
        </details>
      ) : null}
    </main>
  );
}
