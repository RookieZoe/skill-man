import { useCallback, useState } from "react";

import type {
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
  ExistingHomeRecoveryPlan,
  RecoveryProfileFact,
} from "../../app/catalog-client";
import { LanguageControl } from "../locale/LanguageControl";
import { errorMessageKey, errorMessageParams } from "../locale/messages";
import { useLocale } from "../locale/LocaleProvider";

interface DefaultHomeRecoveryViewProps {
  client: CatalogClient;
  snapshot: Extract<
    BootstrapSnapshot,
    { state: "default_home_recovery_offer" }
  >;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
}

type Step =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "preview"; plan: ExistingHomeRecoveryPlan }
  | { kind: "error"; error: CommandFailure };

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
 * The default-path recovery route. Its only forward action prepares the
 * existing read-only Recovery Plan; binding choices and custom path scanning
 * are intentionally absent so this offer cannot be bypassed.
 */
export function DefaultHomeRecoveryView({
  client,
  snapshot,
  onSnapshot,
}: DefaultHomeRecoveryViewProps) {
  const { t } = useLocale();
  const [step, setStep] = useState<Step>({ kind: "idle" });

  const prepare = useCallback(async () => {
    setStep({ kind: "busy" });
    try {
      setStep({
        kind: "preview",
        plan: await client.prepareExistingHomeRecovery(snapshot.path),
      });
    } catch (error) {
      setStep({ kind: "error", error: error as CommandFailure });
    }
  }, [client, snapshot.path]);

  const cancel = useCallback(
    async (planToken: string) => {
      setStep({ kind: "busy" });
      try {
        await client.cancelExistingHomeRecovery(planToken);
        // The native snapshot remains Offer because cancelling a preview is
        // zero-write. Keep its route rather than falling back to binding.
        setStep({ kind: "idle" });
      } catch (error) {
        setStep({ kind: "error", error: error as CommandFailure });
      }
    },
    [client],
  );

  const confirm = useCallback(
    async (planToken: string) => {
      setStep({ kind: "busy" });
      try {
        onSnapshot(await client.confirmExistingHomeRecovery(planToken));
        setStep({ kind: "idle" });
      } catch (error) {
        setStep({ kind: "error", error: error as CommandFailure });
      }
    },
    [client, onSnapshot],
  );

  if (step.kind === "busy") {
    return (
      <RouteShell
        title={t("bootstrap.home.binding_in_progress_title")}
        summary={t("bootstrap.home.binding_in_progress_summary")}
      />
    );
  }

  if (step.kind === "error") {
    return (
      <RouteShell
        title={t("bootstrap.home.failed_title")}
        summary={t(
          errorMessageKey(step.error.error),
          errorMessageParams(step.error.error),
        )}
        diagnostic={step.error.diagnostic ?? undefined}
        action={{
          label: t("bootstrap.home.back"),
          onClick: () => setStep({ kind: "idle" }),
        }}
      />
    );
  }

  if (step.kind === "preview") {
    const { plan } = step;
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
        <div className="bootstrap-actions">
          <button type="button" onClick={() => void confirm(plan.planToken)}>
            {t("bootstrap.recovery.confirm")}
          </button>
          <button type="button" onClick={() => void cancel(plan.planToken)}>
            {t("bootstrap.home.back")}
          </button>
        </div>
        <LanguageControl />
      </main>
    );
  }

  return (
    <RouteShell
      title={t("bootstrap.default_recovery.offer_title")}
      summary={t("bootstrap.default_recovery.offer_summary", {
        path: snapshot.path,
      })}
      action={{
        label: t("bootstrap.default_recovery.review"),
        onClick: () => void prepare(),
      }}
    />
  );
}

function RouteShell({
  title,
  summary,
  diagnostic,
  action,
}: {
  title: string;
  summary: string;
  diagnostic?: { code: string; message: string };
  action?: { label: string; onClick: () => void };
}) {
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
      {action ? (
        <button type="button" onClick={action.onClick}>
          {action.label}
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
      <LanguageControl />
    </main>
  );
}
