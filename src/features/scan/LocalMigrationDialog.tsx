import { useState } from "react";
import { OperationNotice } from "../../ui/OperationNotice";
import type {
  AdoptPlan,
  AdoptResult,
  CatalogClient,
} from "../../app/catalog-client";
import { useModalFocus } from "../../ui/useModalFocus";
import { useLocale } from "../locale/LocaleProvider";

export function LocalMigrationDialog({
  client,
  entityRef,
  generation,
  name,
  pickDirectory,
  formatError,
  onResolved,
  onClose,
}: {
  client: CatalogClient;
  entityRef: string;
  generation: number;
  name: string;
  pickDirectory: () => Promise<string | null>;
  formatError: (error: unknown) => string;
  onResolved: (managed: boolean) => void;
  onClose: () => void;
}) {
  const { t } = useLocale();
  const [parent, setParent] = useState("");
  const [plan, setPlan] = useState<AdoptPlan | null>(null);
  const [result, setResult] = useState<AdoptResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [undone, setUndone] = useState(false);
  function message(reason: unknown) {
    return (reason as { error?: { code?: string } })?.error?.code ===
      "plan_stale"
      ? t("scan.migration.stale")
      : formatError(reason);
  }
  async function close() {
    if (busy) return;
    setBusy(true);
    try {
      if (result) await client.finalizeAdopt(result.operationId);
      else if (plan) await client.cancelAdopt(plan.planToken);
      onClose();
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  }
  const modalRef = useModalFocus<HTMLElement>({
    busy,
    onClose: () => void close(),
  });
  async function choose() {
    setBusy(true);
    setError(null);
    try {
      const value = await pickDirectory();
      if (!value) return;
      if (plan) await client.cancelAdopt(plan.planToken);
      setPlan(null);
      setParent(value);
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  }
  async function preview() {
    setBusy(true);
    setError(null);
    try {
      if (plan) await client.cancelAdopt(plan.planToken);
      setPlan(null);
      setPlan(
        await client.planAdopt(generation, [
          {
            entityRef,
            action: "local_link_with_move",
            destinationParent: parent,
          },
        ]),
      );
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  }
  async function apply() {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      const value = await client.applyAdopt(plan.planToken);
      setResult(value);
      if (value.items.some((item) => item.adopted)) onResolved(true);
    } catch (reason) {
      setError(message(reason));
      setPlan(null);
    } finally {
      setBusy(false);
    }
  }
  async function undo() {
    if (!result) return;
    setBusy(true);
    setError(null);
    try {
      const value = await client.undoAdopt(result.operationId);
      if (value.items.some((item) => item.undone)) {
        setUndone(true);
        onResolved(false);
      }
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="activation-sheet-backdrop">
      <section
        ref={modalRef}
        className="activation-sheet local-migration-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("scan.migration.title", { name })}
        tabIndex={-1}
        aria-busy={busy}
      >
        <header className="activation-sheet-heading">
          <h2>{t("scan.migration.title", { name })}</h2>
          <OperationNotice busy={busy} />
        </header>
        <div className="enable-sheet-body">
          {!result && (
            <div className="scan-migration-preview import-source-field">
              <label htmlFor="migration-parent">
                {t("scan.migration.parent")}
              </label>
              <div className="import-directory-picker">
                <input
                  id="migration-parent"
                  readOnly
                  value={parent}
                  placeholder={t("scan.migration.choose")}
                />
                <button
                  type="button"
                  className="toolbar-button"
                  disabled={busy}
                  onClick={() => void choose()}
                >
                  {t("scan.migration.choose")}
                </button>
              </div>
            </div>
          )}
          {plan &&
            !result &&
            plan.items.map((item) => (
              <div className="scan-migration-preview" key={item.entityRef}>
                <dl>
                  <dt>{t("scan.migration.source")}</dt>
                  <dd>{item.canonicalEntity}</dd>
                  <dt>{t("scan.migration.destination")}</dt>
                  <dd>{item.finalEntityPath}</dd>
                  <dt>{t("scan.migration.links")}</dt>
                  <dd>
                    {item.activations.map((link) => (
                      <div key={link.entryPath}>
                        {link.entryPath} → {link.targetPath}
                      </div>
                    ))}
                  </dd>
                </dl>
                <p>{t("scan.migration.confirm")}</p>
              </div>
            ))}
          {result && (
            <div role="status">
              {undone
                ? t("scan.ledger.adoptUndone", { name })
                : result.items.map((item) => (
                    <p key={item.skillId}>
                      {item.adopted
                        ? t("scan.ledger.adoptAdopted", {
                            name: item.directoryName,
                          })
                        : t("scan.ledger.adoptFailed", {
                            name: item.directoryName,
                            detail:
                              item.error ?? t("library.adopt.failed_unknown"),
                          })}
                    </p>
                  ))}
            </div>
          )}
          {error && (
            <p role="alert" className="scan-ledger-error">
              {error}
            </p>
          )}
        </div>
        <footer className="activation-sheet-actions">
          <button type="button" disabled={busy} onClick={() => void close()}>
            {result ? t("scan.migration.close") : t("agents.sheet.cancel")}
          </button>
          {!result &&
            (!plan ? (
              <button
                type="button"
                disabled={busy || !parent}
                onClick={() => void preview()}
              >
                {t("scan.ledger.adoptPlan")}
              </button>
            ) : (
              <button
                type="button"
                disabled={busy || !plan.canApply}
                onClick={() => void apply()}
                className="activation-confirm-button"
              >
                {t("scan.migration.apply")}
              </button>
            ))}
          {result?.undoAvailable && !undone && (
            <button type="button" disabled={busy} onClick={() => void undo()}>
              {t("scan.ledger.adoptUndo")}
            </button>
          )}
        </footer>
      </section>
    </div>
  );
}
