import { useEffect, useState } from "react";
import type {
  CatalogClient,
  GlobalTargetGroup,
  SkillSummary,
} from "../../app/catalog-client";
import { useLocale } from "../locale/LocaleProvider";
import { commandErrorMessage } from "../locale/messages";
import { useModalFocus } from "../../ui/useModalFocus";
import { OperationNotice } from "../../ui/OperationNotice";

type Entry = { skillId: string; name: string; group: GlobalTargetGroup };

export function BatchDisableSheet({
  client,
  skills,
  onCatalogChanged,
  onClose,
}: {
  client: CatalogClient;
  skills: SkillSummary[];
  onCatalogChanged?: () => Promise<void>;
  onClose: () => void;
}) {
  const { t } = useLocale();
  // Freeze the user's selection for this confirmation, including after refresh.
  const [selection] = useState(skills);
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);
  const [completed, setCompleted] = useState<number | null>(null);
  const busy = applying || (entries === null && !error);
  const ref = useModalFocus<HTMLElement>({
    busy,
    onClose,
    focusKey: `${busy}:${completed}`,
  });
  useEffect(() => {
    let active = true;
    Promise.all(selection.map((skill) => client.listTargetGroups(skill.id)))
      .then((snapshots) => {
        if (active)
          setEntries(
            snapshots.flatMap((snapshot) =>
              snapshot.groups
                .filter((group) => group.desired)
                .map((group) => ({
                  skillId: snapshot.skillId,
                  name: snapshot.skillName,
                  group,
                })),
            ),
          );
      })
      .catch((cause) => {
        if (active) setError(commandErrorMessage(cause, t));
      });
    return () => {
      active = false;
    };
  }, [client, selection, t]);

  async function apply() {
    if (!entries?.length || applying || completed !== null) return;
    setApplying(true);
    setError(null);
    let count = 0;
    try {
      for (const entry of entries) {
        // Each plan is revalidated after the preceding write. Never reuse a
        // stale multi-plan snapshot or bypass Core's ownership checks.
        const plan = await client.planGlobalLifecycle(
          entry.skillId,
          entry.group.targetRootId,
          "disable",
        );
        if (
          plan.cells.length !== 1 ||
          plan.cells[0].action !== "disable" ||
          plan.cells[0].skillId !== entry.skillId ||
          plan.cells[0].targetRootId !== entry.group.targetRootId ||
          !["ready", "no_op"].includes(plan.cells[0].eligibility)
        )
          throw new Error(t("batchDisable.blocked", { skill: entry.name }));
        if (plan.cells[0].eligibility === "no_op") {
          count++;
          continue;
        }
        const result = await client.applyGlobalEnable(plan.planToken);
        const ok =
          result.cells.length === 1 &&
          result.cells[0].skillId === entry.skillId &&
          result.cells[0].targetRootId === entry.group.targetRootId &&
          ["succeeded", "no_op"].includes(result.cells[0].outcome);
        if (ok) count++;
        await client.finalizeGlobalEnable(result.operationId);
        if (!ok)
          throw new Error(t("batchDisable.blocked", { skill: entry.name }));
      }
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setCompleted(count);
      try {
        await onCatalogChanged?.();
      } catch {
        setError(
          (current) => current ?? t("operation.background.refresh_failed"),
        );
      }
      setApplying(false);
    }
  }
  return (
    <div className="activation-sheet-backdrop batch-disable-backdrop">
      <section
        ref={ref}
        className="activation-sheet batch-disable-sheet"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        aria-labelledby="batch-disable-title"
      >
        <header className="batch-disable-heading">
          <h2 id="batch-disable-title">{t("shelf.disable")}</h2>
          <OperationNotice
            busy={busy}
            label={t(
              applying ? "batchDisable.running" : "batchDisable.loading",
            )}
          />
        </header>
        <div className="batch-disable-body">
          <p>{t("batchDisable.description")}</p>
          {entries?.length === 0 && <p>{t("batchDisable.empty")}</p>}
          <ul>
            {entries?.map((entry) => (
              <li key={`${entry.skillId}|${entry.group.targetRootId}`}>
                <strong>{entry.name}</strong>
                <span>
                  {entry.group.consumers
                    .map((agent) => agent.agentName)
                    .join(", ")}
                </span>
                <code>{entry.group.configuredPath}</code>
              </li>
            ))}
          </ul>
          {completed !== null && (
            <p role="status">
              {t("batchDisable.result", {
                completed,
                total: entries?.length ?? 0,
              })}
            </p>
          )}
          {error && <p role="alert">{error}</p>}
        </div>
        <footer className="activation-sheet-actions">
          <button
            type="button"
            className="toolbar-button"
            disabled={busy}
            onClick={onClose}
          >
            {t("enable.broken.close")}
          </button>
          {completed === null && (
            <button
              type="button"
              className="toolbar-button"
              disabled={busy || !!error || !entries?.length}
              onClick={() => void apply()}
            >
              {t("batchDisable.confirm")}
            </button>
          )}
        </footer>
      </section>
    </div>
  );
}
