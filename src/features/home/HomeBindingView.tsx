import { useCallback, useState } from "react";

import type {
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
  ExistingHomeRecoveryPlan,
  HomeCandidate,
  RecoveryProfileFact,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { errorMessageKey, errorMessageParams } from "../locale/messages";
import { formatByteSize } from "../locale/messages";
import { LanguageControl } from "../locale/LanguageControl";

export type HomePicker = () => Promise<string | null>;

/** Default directory picker: the native open dialog inside Tauri. */
export async function defaultHomePicker(): Promise<string | null> {
  if (!("__TAURI_INTERNALS__" in window)) {
    return null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({
    directory: true,
    multiple: false,
    title: undefined,
  });
  return typeof result === "string" ? result : null;
}

interface HomeBindingViewProps {
  client: CatalogClient;
  snapshot:
    | Extract<BootstrapSnapshot, { state: "unconfigured" }>
    | Extract<BootstrapSnapshot, { state: "abandoned" }>
    | Extract<BootstrapSnapshot, { state: "legacy_detected" }>
    | Extract<BootstrapSnapshot, { state: "home_candidate_pending" }>;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
  pickDirectory?: HomePicker;
}

type Step =
  | { kind: "idle" }
  | { kind: "busy"; action: string }
  | { kind: "confirm"; candidate: HomeCandidate }
  | { kind: "recovery_preview"; plan: ExistingHomeRecoveryPlan }
  | { kind: "error"; error: CommandFailure; action: string };

const recoveryFactMessageKeys: Record<
  RecoveryProfileFact,
  | "bootstrap.recovery.fact.marker_catalog_identity"
  | "bootstrap.recovery.fact.standard_layout"
  | "bootstrap.recovery.fact.catalog_integrity"
  | "bootstrap.recovery.fact.catalog_foreign_keys"
  | "bootstrap.recovery.fact.catalog_capabilities"
> = {
  marker_catalog_identity: "bootstrap.recovery.fact.marker_catalog_identity",
  standard_layout: "bootstrap.recovery.fact.standard_layout",
  catalog_integrity: "bootstrap.recovery.fact.catalog_integrity",
  catalog_foreign_keys: "bootstrap.recovery.fact.catalog_foreign_keys",
  catalog_capabilities: "bootstrap.recovery.fact.catalog_capabilities",
};

/**
 * The Home Binding wizard (spec §5.3–§5.4): explicit Use Default / Choose…
 * with a mandatory confirmation, plus Continue/Cancel for an interrupted
 * candidate. The native snapshot stays the authority — this component only
 * carries the ephemeral selection and confirmation state.
 */
export function HomeBindingView({
  client,
  snapshot,
  onSnapshot,
  pickDirectory = defaultHomePicker,
}: HomeBindingViewProps) {
  const { t, locale } = useLocale();
  const [step, setStep] = useState<Step>({ kind: "idle" });
  const [busy, setBusy] = useState(false);

  const isLegacy = snapshot.state === "legacy_detected";
  const isPending = snapshot.state === "home_candidate_pending";
  const isAbandoned = snapshot.state === "abandoned";

  const fail = useCallback((error: CommandFailure, action: string) => {
    setStep({ kind: "error", error, action });
  }, []);

  const prepare = useCallback(
    async (path: string, action: string) => {
      setBusy(true);
      setStep({ kind: "busy", action });
      try {
        const candidate = await client.prepareHome(path);
        setStep({ kind: "confirm", candidate });
      } catch (error) {
        fail(error as CommandFailure, action);
      } finally {
        setBusy(false);
      }
    },
    [client, fail],
  );

  const confirmCandidate = useCallback(
    async (token: string) => {
      setBusy(true);
      setStep({ kind: "busy", action: "confirm" });
      try {
        onSnapshot(await client.confirmHome(token));
        setStep({ kind: "idle" });
      } catch (error) {
        fail(error as CommandFailure, "confirm");
      } finally {
        setBusy(false);
      }
    },
    [client, fail, onSnapshot],
  );

  const resume = useCallback(
    async (operationId: string, cancel: boolean) => {
      setBusy(true);
      setStep({ kind: "busy", action: cancel ? "cancel" : "continue" });
      try {
        const next = cancel
          ? await client.cancelCandidate(operationId)
          : await client.continueCandidate(operationId);
        onSnapshot(next);
        setStep({ kind: "idle" });
      } catch (error) {
        fail(error as CommandFailure, cancel ? "cancel" : "continue");
      } finally {
        setBusy(false);
      }
    },
    [client, fail, onSnapshot],
  );

  const choose = useCallback(async () => {
    const path = await pickDirectory();
    if (!path) {
      setStep({ kind: "idle" });
      return;
    }
    await prepare(path, "choose");
  }, [pickDirectory, prepare]);

  const recoverExisting = useCallback(async () => {
    const path = await pickDirectory();
    if (!path) {
      setStep({ kind: "idle" });
      return;
    }
    setBusy(true);
    setStep({ kind: "busy", action: "recover" });
    try {
      setStep({
        kind: "recovery_preview",
        plan: await client.prepareExistingHomeRecovery(path),
      });
    } catch (error) {
      fail(error as CommandFailure, "recover");
    } finally {
      setBusy(false);
    }
  }, [client, fail, pickDirectory]);

  const cancelRecovery = useCallback(
    async (planToken: string) => {
      setBusy(true);
      try {
        await client.cancelExistingHomeRecovery(planToken);
        setStep({ kind: "idle" });
      } catch (error) {
        fail(error as CommandFailure, "cancel_recovery");
      } finally {
        setBusy(false);
      }
    },
    [client, fail],
  );

  const dismissError = useCallback(() => {
    setStep({ kind: "idle" });
  }, []);

  if (step.kind === "busy") {
    return (
      <main
        role="status"
        aria-live="polite"
        className="bootstrap-route"
        data-bootstrap-route
      >
        <h1>{t("bootstrap.home.binding_in_progress_title")}</h1>
        <p>{t("bootstrap.home.binding_in_progress_summary")}</p>
        <LanguageControl />
      </main>
    );
  }

  if (step.kind === "error") {
    return (
      <main
        role="status"
        aria-live="polite"
        className="bootstrap-route"
        data-bootstrap-route
      >
        <h1>{t("bootstrap.home.failed_title")}</h1>
        <p>
          {t(
            errorMessageKey(step.error.error),
            errorMessageParams(step.error.error),
          )}
        </p>
        {step.error.diagnostic ? (
          <details className="bootstrap-diagnostic">
            <summary>{t("bootstrap.technical_details")}</summary>
            <dl>
              <dt>{t("bootstrap.code")}</dt>
              <dd>{step.error.diagnostic.code}</dd>
              <dt>{t("bootstrap.detail")}</dt>
              <dd>{step.error.diagnostic.message}</dd>
            </dl>
          </details>
        ) : null}
        <button type="button" onClick={dismissError}>
          {t("bootstrap.home.back")}
        </button>
        <LanguageControl />
      </main>
    );
  }

  if (step.kind === "confirm") {
    const candidate = step.candidate;
    return (
      <main
        role="status"
        aria-live="polite"
        className="bootstrap-route"
        data-bootstrap-route
      >
        <h1>{t("bootstrap.home.confirm_title")}</h1>
        <p>{t("bootstrap.home.confirm_summary", { path: candidate.path })}</p>
        <p>
          {candidate.mode === "legacy_in_place"
            ? t("bootstrap.home.mode_in_place")
            : candidate.mode === "legacy_copy"
              ? t("bootstrap.home.mode_copy")
              : t("bootstrap.home.mode_fresh")}
        </p>
        <p className="bootstrap-home-facts">
          {t("bootstrap.home.available_space", {
            bytes: formatByteSize(locale, candidate.availableBytes),
          })}
        </p>
        <div className="bootstrap-actions">
          <button
            type="button"
            disabled={busy}
            onClick={() => confirmCandidate(candidate.token)}
          >
            {t("bootstrap.home.confirm_bind")}
          </button>
          <button type="button" disabled={busy} onClick={dismissError}>
            {t("bootstrap.home.back")}
          </button>
        </div>
        <LanguageControl />
      </main>
    );
  }

  if (step.kind === "recovery_preview") {
    const plan = step.plan;
    return (
      <main
        role="status"
        aria-live="polite"
        className="bootstrap-route"
        data-bootstrap-route
      >
        <h1>{t("bootstrap.recovery.preview_title")}</h1>
        <p>{t("bootstrap.recovery.preview_summary")}</p>
        <dl className="bootstrap-home-facts">
          <dt>{t("bootstrap.recovery.path")}</dt>
          <dd>{plan.path}</dd>
          <dt>{t("bootstrap.recovery.home_id")}</dt>
          <dd>{plan.homeId}</dd>
          <dt>{t("bootstrap.recovery.created_at")}</dt>
          <dd>{plan.createdAt}</dd>
        </dl>
        <ul className="bootstrap-home-facts">
          {plan.facts.map((fact) => (
            <li key={fact}>{t(recoveryFactMessageKeys[fact])}</li>
          ))}
        </ul>
        <p>{t("bootstrap.recovery.confirmation_next")}</p>
        <button
          type="button"
          disabled={busy}
          onClick={() => void cancelRecovery(plan.planToken)}
        >
          {t("bootstrap.home.back")}
        </button>
        <LanguageControl />
      </main>
    );
  }

  if (isPending) {
    return (
      <main
        role="status"
        aria-live="polite"
        className="bootstrap-route"
        data-bootstrap-route
      >
        <h1>{t("bootstrap.route.candidate_title")}</h1>
        <p>{t("bootstrap.route.candidate_summary", { path: snapshot.path })}</p>
        <p>{t("bootstrap.home.pending_explanation")}</p>
        <div className="bootstrap-actions">
          <button
            type="button"
            disabled={busy}
            onClick={() => resume(snapshot.operationId, false)}
          >
            {t("bootstrap.home.continue")}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => resume(snapshot.operationId, true)}
          >
            {t("bootstrap.home.cancel")}
          </button>
        </div>
        <LanguageControl />
      </main>
    );
  }

  return (
    <main
      role="status"
      aria-live="polite"
      className="bootstrap-route"
      data-bootstrap-route
    >
      <h1>
        {isAbandoned
          ? t("bootstrap.route.abandoned_title")
          : isLegacy
            ? t("bootstrap.route.legacy_title")
            : t("bootstrap.route.unconfigured_title")}
      </h1>
      <p>
        {isAbandoned
          ? t("bootstrap.route.abandoned_summary", { path: snapshot.path })
          : isLegacy
            ? t("bootstrap.route.legacy_summary", { path: snapshot.path })
            : t("bootstrap.route.unconfigured_summary")}
      </p>
      {isAbandoned ? <p>{t("bootstrap.home.abandoned_notice")}</p> : null}
      {isLegacy ? <p>{t("bootstrap.home.legacy_explanation")}</p> : null}
      <div className="bootstrap-actions">
        <button
          type="button"
          disabled={busy || isAbandoned}
          onClick={() => prepare(isLegacy ? snapshot.path : "", "default")}
        >
          {isAbandoned
            ? t("bootstrap.home.use_default")
            : isLegacy
              ? t("bootstrap.home.use_default_legacy", { path: snapshot.path })
              : t("bootstrap.home.use_default")}
        </button>
        <button type="button" disabled={busy} onClick={choose}>
          {t("bootstrap.home.choose")}
        </button>
        {!isLegacy && !isAbandoned ? (
          <button
            type="button"
            disabled={busy}
            onClick={() => void recoverExisting()}
          >
            {t("bootstrap.recovery.choose")}
          </button>
        ) : null}
      </div>
      <LanguageControl />
    </main>
  );
}
