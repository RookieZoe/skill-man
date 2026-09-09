import { OperationNotice } from "../../ui/OperationNotice";
import { useCallback, useEffect, useState } from "react";

import type {
  AbandonPreview,
  BootstrapSnapshot,
  CatalogClient,
  CommandFailure,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { LanguageControl } from "../locale/LanguageControl";
import { errorMessageKey, errorMessageParams } from "../locale/messages";

export interface AbandonFlowProps {
  client: CatalogClient;
  onSnapshot: (snapshot: BootstrapSnapshot) => void;
  onClose: () => void;
}

/**
 * Abandon Home and Start New (ADR-0012 §6): the high-friction escape hatch.
 * Two explicit confirmations — the typed Home ID must match exactly, then a
 * second explicit confirm — before the locator CAS commits. The old Home,
 * its Activations and the locale are never touched.
 */
export function AbandonFlow({ client, onSnapshot, onClose }: AbandonFlowProps) {
  const { t } = useLocale();
  const [preview, setPreview] = useState<AbandonPreview | null>(null);
  const [typed, setTyped] = useState("");
  const [step, setStep] = useState<"preview" | "confirm">("preview");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CommandFailure | null>(null);

  const plan = useCallback(() => {
    client
      .planAbandon()
      .then((next) => {
        setPreview(next);
        setTyped("");
        setStep("preview");
        setError(null);
      })
      .catch((failure) => {
        setError(failure as CommandFailure);
      });
  }, [client]);

  useEffect(() => {
    plan();
  }, [plan]);

  // Escape dismisses the dialog unless an Abandon is in flight; the key
  // listener lives on the window so it works regardless of focus.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const confirmAbandon = async () => {
    if (!preview) return;
    setBusy(true);
    setError(null);
    try {
      const snapshot = await client.applyAbandon(preview.planToken, typed);
      onSnapshot(snapshot);
      onClose();
    } catch (failure) {
      setError(failure as CommandFailure);
    } finally {
      setBusy(false);
    }
  };

  const matches = preview !== null && typed === preview.homeId;

  return (
    <div className="overlay-backdrop" role="presentation">
      <div
        className="abandon-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="abandon-title"
      >
        <OperationNotice busy={busy} />
        {!preview && !error ? <p>{t("lifecycle.abandon_title")}</p> : null}
        {error ? (
          <>
            <h2 id="abandon-title">{t("lifecycle.abandon_failed")}</h2>
            <p
              className="recovery-notice recovery-notice--error operation-message operation-message--error"
              role="alert"
            >
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
            <div className="bootstrap-actions">
              <button type="button" onClick={plan}>
                {t("lifecycle.abandon_start_over")}
              </button>
              <button type="button" onClick={onClose}>
                {t("lifecycle.abandon_close")}
              </button>
            </div>
          </>
        ) : preview ? (
          <>
            <h2 id="abandon-title">{t("lifecycle.abandon_title")}</h2>
            <p>
              {t("lifecycle.abandon_summary", {
                homeId: preview.homeId,
                path: preview.path,
              })}
            </p>
            <p>{t("lifecycle.abandon_effects")}</p>
            <label className="abandon-type">
              <span>{t("lifecycle.abandon_type_home_id")}</span>
              <input
                type="text"
                value={typed}
                disabled={busy}
                onChange={(event) => setTyped(event.target.value)}
                aria-describedby="abandon-hint"
              />
              <span id="abandon-hint" className="abandon-hint">
                {t("lifecycle.abandon_type_home_id_hint", {
                  homeId: preview.homeId,
                })}
              </span>
            </label>
            <div className="bootstrap-actions">
              {step === "preview" ? (
                <button
                  type="button"
                  disabled={busy || !matches}
                  onClick={() => setStep("confirm")}
                >
                  {t("lifecycle.abandon_continue")}
                </button>
              ) : (
                <button
                  type="button"
                  disabled={busy || !matches}
                  onClick={() => void confirmAbandon()}
                >
                  {busy
                    ? t("lifecycle.abandon_confirm_busy")
                    : t("lifecycle.abandon_confirm")}
                </button>
              )}
              <button
                type="button"
                disabled={busy}
                onClick={
                  step === "confirm" ? () => setStep("preview") : onClose
                }
              >
                {step === "confirm"
                  ? t("lifecycle.abandon_back")
                  : t("lifecycle.abandon_close")}
              </button>
            </div>
          </>
        ) : null}
        <LanguageControl />
      </div>
    </div>
  );
}
