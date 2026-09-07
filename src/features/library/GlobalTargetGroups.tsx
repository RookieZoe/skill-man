import { useBackgroundOperations } from "../../ui/BackgroundOperations";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

import type {
  CatalogClient,
  EnableCell,
  EnableResult,
  GlobalTargetGroup,
  GlobalTargetGroupSnapshot,
} from "../../app/catalog-client";
import { LockIcon } from "../../ui/icons";
import { errorMessageKey, errorMessageParams } from "../locale/messages";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import { GlobalEnableSheet } from "./GlobalEnableSheet";

/**
 * Agent Inspector Target group cards (spec §7.4): one card per canonical
 * Target, shared Agents merged with a single switch, one desired/observed
 * state and one Repair action. A Target that is missing or unverifiable gets
 * its own card exposing only `open_agent_management`; the permanent footer
 * note keeps project-level one-shot links clearly out of scope.
 */
export function GlobalTargetGroupsPanel({
  ref,
  dialog,
  skillId,
  client,
  onOpenAgentManagement,
  onOpenAdopt,
  onOpenBrokenDisable,
  onOverlayChange,
  refreshToken,
  onCatalogChanged,
}: {
  ref: React.Ref<HTMLElement | null>;
  dialog: boolean;
  skillId: string;
  client: CatalogClient;
  onOpenAgentManagement: () => void;
  onOpenAdopt?: (cell: EnableCell) => void;
  onOpenBrokenDisable?: (trigger: HTMLButtonElement) => void;
  onOverlayChange?: (open: boolean) => void;
  refreshToken?: number;
  onCatalogChanged?: () => Promise<void>;
}) {
  const { t } = useLocale();
  const notifications = useBackgroundOperations();
  const [snapshot, setSnapshot] = useState<GlobalTargetGroupSnapshot | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [busyGroupId, setBusyGroupId] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<EnableResult | null>(null);
  const [lastGroupId, setLastGroupId] = useState<string | null>(null);
  const [sheetInitial, setSheetInitial] = useState<string[] | null>(null);
  const [sheetOpener, setSheetOpener] = useState<HTMLElement | null>(null);

  useEffect(() => {
    onOverlayChange?.(sheetInitial !== null);
    return () => onOverlayChange?.(false);
  }, [onOverlayChange, sheetInitial]);

  async function load() {
    try {
      const next = await client.listTargetGroups(skillId);
      setSnapshot(next);
      setError(null);
      return true;
    } catch (cause) {
      setError(targetGroupErrorMessage(cause, t));
      return false;
    }
  }

  useEffect(() => {
    let current = true;
    client
      .listTargetGroups(skillId)
      .then((next) => {
        if (!current) return;
        setSnapshot(next);
        setError(null);
      })
      .catch((cause) => {
        if (current) setError(targetGroupErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, skillId, t]);

  useEffect(() => {
    if (refreshToken === undefined || refreshToken === 0) return;
    let current = true;
    client
      .listTargetGroups(skillId)
      .then((next) => {
        if (!current) return;
        setSnapshot(next);
        setError(null);
      })
      .catch((cause) => {
        if (current) setError(targetGroupErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, refreshToken, skillId, t]);

  useEffect(() => {
    let current = true;
    let unlisten: (() => void) | null = null;
    client
      .listenObservationChanged((payload) => {
        if (!current) return;
        if (
          payload.activationHealth === null &&
          payload.startupProbe === null
        ) {
          return;
        }
        void client
          .listTargetGroups(skillId)
          .then((next) => {
            if (!current) return;
            setSnapshot(next);
            setError(null);
          })
          .catch((cause) => {
            if (current) setError(targetGroupErrorMessage(cause, t));
          });
      })
      .then((stop) => {
        if (!current) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      current = false;
      unlisten?.();
    };
  }, [client, skillId, t]);

  async function onToggle(group: GlobalTargetGroup) {
    if (group.action !== "none") {
      onOpenAgentManagement();
      return;
    }
    setBusyGroupId(group.targetRootId);
    setError(null);
    const action = group.desired ? "disable" : "enable";
    const context = {
      skill: snapshot?.skillName ?? skillId,
      target:
        group.consumers.map((agent) => agent.agentName).join(", ") ||
        group.configuredPath,
    };
    const noticeId = notifications.begin({
      title: t(`operation.activation.${action}_running`),
      detail: t("operation.activation.context", context),
    });
    try {
      const plan = await client.planGlobalLifecycle(
        skillId,
        group.targetRootId,
        action,
      );
      const cell = plan.cells[0];
      if (cell.eligibility === "conflict") {
        notifications.dismiss(noticeId);
        openEnableSheet([group.targetRootId]);
        return;
      }
      const applied = await client.applyGlobalEnable(plan.planToken);
      setLastResult(applied);
      setLastGroupId(group.targetRootId);
      const refreshed = await Promise.all([load(), onCatalogChanged?.()]).then(
        ([loaded]) => loaded,
        () => false,
      );
      const complete = applied.cells.every(
        (cell) => cell.outcome === "succeeded" || cell.outcome === "no_op",
      );
      notifications.finish(noticeId, {
        title: t(
          complete
            ? `operation.activation.${action}_done`
            : "operation.background.partial",
        ),
        detail: [
          t("operation.activation.context", context),
          !refreshed
            ? t("operation.background.refresh_failed")
            : !complete
              ? t("operation.activation.partial_next")
              : action === "disable"
                ? t("operation.activation.disable_next")
                : t("operation.activation.enable_next"),
        ].join(" "),
        state: complete ? "completed" : "partial",
      });
    } catch (cause) {
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: targetGroupErrorMessage(cause, t),
        state: "failed",
      });
    } finally {
      setBusyGroupId(null);
    }
  }

  async function onRepair(group: GlobalTargetGroup) {
    setBusyGroupId(group.targetRootId);
    setError(null);
    const noticeId = notifications.begin({
      title: t("operation.activation.repair_running"),
      detail: t("operation.activation.context", {
        skill: snapshot?.skillName ?? skillId,
        target: group.configuredPath,
      }),
    });
    try {
      const plan = await client.planGlobalLifecycle(
        skillId,
        group.targetRootId,
        "repair",
      );
      const applied = await client.applyGlobalEnable(plan.planToken);
      setLastResult(applied);
      setLastGroupId(group.targetRootId);
      const refreshed = await Promise.all([load(), onCatalogChanged?.()]).then(
        ([loaded]) => loaded,
        () => false,
      );
      const complete = applied.cells.every(
        (cell) => cell.outcome === "succeeded" || cell.outcome === "no_op",
      );
      notifications.finish(noticeId, {
        title: t(
          complete
            ? "operation.activation.repair_done"
            : "operation.background.partial",
        ),
        detail: t(
          refreshed
            ? complete
              ? "operation.activation.repair_next"
              : "operation.activation.partial_next"
            : "operation.background.refresh_failed",
        ),
        state: complete ? "completed" : "partial",
      });
    } catch (cause) {
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: targetGroupErrorMessage(cause, t),
        state: "failed",
      });
    } finally {
      setBusyGroupId(null);
    }
  }

  async function onUndo() {
    if (!lastResult) {
      return;
    }
    setBusyGroupId(lastGroupId);
    const noticeId = notifications.begin({
      title: t("operation.activation.undo_running"),
      detail: t("operation.background.working"),
    });
    try {
      const undone = await client.undoGlobalEnable(lastResult.operationId);
      setLastResult(null);
      setLastGroupId(null);
      const refreshed = await Promise.all([load(), onCatalogChanged?.()]).then(
        ([loaded]) => loaded,
        () => false,
      );
      const complete = undone.cells.every((cell) => cell.undone);
      notifications.finish(noticeId, {
        title: t(
          complete
            ? "operation.activation.undo_done"
            : "operation.background.partial",
        ),
        detail: t(
          refreshed
            ? complete
              ? "operation.activation.undo_next"
              : "operation.activation.partial_next"
            : "operation.background.refresh_failed",
        ),
        state: complete ? "completed" : "partial",
      });
    } catch (cause) {
      notifications.finish(noticeId, {
        title: t("operation.background.failed"),
        detail: targetGroupErrorMessage(cause, t),
        state: "failed",
      });
    } finally {
      setBusyGroupId(null);
    }
  }

  function openEnableSheet(initial: string[]) {
    setSheetOpener(
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null,
    );
    setSheetInitial(initial);
  }

  return (
    <aside
      ref={ref}
      id={dialog ? "agent-inspector-dialog" : undefined}
      className="agent-inspector"
      aria-label={t("library.activation.target_groups")}
      role={dialog ? "dialog" : undefined}
      aria-modal={dialog ? true : undefined}
      tabIndex={-1}
    >
      <div className="panel-heading inspector-heading">
        <div>
          <span className="eyebrow">{t("library.activation.eyebrow")}</span>
          <h2>{t("library.activation.target_groups")}</h2>
        </div>
      </div>
      <p className="inspector-intro">
        {t("library.activation.target_groups_body")}
      </p>

      {error !== null && (
        <p className="inspector-error" role="alert">
          {t("inspector.diagnosticNote", { detail: error })}
        </p>
      )}

      {snapshot === null ? null : snapshot.groups.length === 0 ? (
        <div className="activation-target-placeholder">
          <span aria-hidden="true" />
          <strong>{t("library.activation.target_groups_empty")}</strong>
          <small>{t("library.activation.target_groups_hint")}</small>
          <button
            type="button"
            className="toolbar-button"
            onClick={onOpenAgentManagement}
          >
            {t("surface.agents")}
          </button>
        </div>
      ) : (
        <ul className="target-group-cards">
          {snapshot.groups.map((group) => (
            <TargetGroupCard
              key={group.targetRootId}
              group={group}
              skillHealth={snapshot.skillHealth}
              busy={busyGroupId === group.targetRootId}
              onToggle={() => void onToggle(group)}
              onRepair={() => void onRepair(group)}
              onOpenAgentManagement={onOpenAgentManagement}
              onOpenBrokenDisable={onOpenBrokenDisable}
            />
          ))}
        </ul>
      )}

      {lastResult !== null && (
        <div className="inspector-result">
          <button
            type="button"
            className="toolbar-button"
            disabled={busyGroupId !== null}
            onClick={() => void onUndo()}
          >
            {t("enable.global.undoOperation")}
          </button>
        </div>
      )}

      <div className="inspector-footnote">
        <LockIcon />
        <span>{t("inspector.projectFootnote")}</span>
      </div>

      {sheetInitial !== null
        ? createPortal(
            <GlobalEnableSheet
              client={client}
              skillId={skillId}
              skillName={snapshot?.skillName ?? skillId}
              initialGroupIds={sheetInitial}
              opener={sheetOpener}
              onAdoptExisting={(cell) => {
                setSheetOpener(null);
                setSheetInitial(null);
                onOpenAdopt?.(cell);
              }}
              onCatalogChanged={onCatalogChanged}
              onClose={() => {
                setSheetInitial(null);
                setSheetOpener(null);
                void load();
              }}
            />,
            document.body,
          )
        : null}
    </aside>
  );
}

function targetGroupErrorMessage(
  reason: unknown,
  t: LocaleContextValue["t"],
): string {
  const failure = reason as {
    error?: { code?: string; directoryName?: string };
    code?: string;
  };
  const error = failure?.error;
  if (error?.code) {
    return t(
      errorMessageKey(error.code),
      errorMessageParams({
        code: error.code,
        directoryName: error.directoryName,
      }),
    );
  }
  if (failure?.code) {
    return t(errorMessageKey(failure.code));
  }
  return t("error.internal");
}

function TargetGroupCard({
  group,
  skillHealth,
  busy,
  onToggle,
  onRepair,
  onOpenAgentManagement,
  onOpenBrokenDisable,
}: {
  group: GlobalTargetGroup;
  skillHealth: GlobalTargetGroupSnapshot["skillHealth"];
  busy: boolean;
  onToggle: () => void;
  onRepair: () => void;
  onOpenAgentManagement: () => void;
  onOpenBrokenDisable?: (trigger: HTMLButtonElement) => void;
}) {
  const { t } = useLocale();
  const consumersLabel = group.consumers
    .map((consumer) => consumer.agentName)
    .join(", ");
  const observedLabel = observedStateLabel(group, t);
  return (
    <li className="target-group-card">
      <div className="target-group-card-head">
        <span className="target-group-path">{group.configuredPath}</span>
        {group.consumers.length > 1 && (
          <span className="target-group-consumer-count">
            {tPluralConsumerCount(group.consumers.length, t)}
          </span>
        )}
      </div>
      <div className="target-group-members">
        {consumersLabel}
        {group.consumers.some(
          (consumer) =>
            !consumer.userConfigured && consumer.compatibility === "unknown",
        ) && (
          <span className="compat-unknown">
            {t("library.activation.compat_unknown")}
          </span>
        )}
      </div>
      {group.action !== "none" ? (
        <div className="target-group-conflict">
          <strong>{t("inspector.conflictCardTitle")}</strong>
          <p>
            {group.availability === "absent"
              ? t("inspector.targetAbsent")
              : t("inspector.targetUnavailable")}
          </p>
          {group.diagnostic !== null && (
            <small>
              {t("inspector.diagnosticNote", { detail: group.diagnostic })}
            </small>
          )}
          <button
            type="button"
            className="toolbar-button"
            onClick={onOpenAgentManagement}
          >
            {t("inspector.openAgentManagement")}
          </button>
        </div>
      ) : skillHealth === "broken" ? (
        <div className="target-group-health-blocked">
          <strong>{t("inspector.brokenCardTitle")}</strong>
          <p>{t("inspector.brokenCardBody")}</p>
          {group.desired && onOpenBrokenDisable ? (
            <button
              type="button"
              className="toolbar-button danger-button"
              disabled={busy}
              onClick={(event) => onOpenBrokenDisable(event.currentTarget)}
            >
              {t("inspector.disableBroken")}
            </button>
          ) : null}
        </div>
      ) : skillHealth === "source_snapshot_mismatch" ? (
        <div className="target-group-health-blocked">
          <strong>{t("inspector.mismatchCardTitle")}</strong>
          <p>{t("inspector.mismatchCardBody")}</p>
          {group.desired ? (
            <button
              type="button"
              className="toolbar-button"
              disabled={busy}
              onClick={onToggle}
            >
              {t("inspector.disableMismatch")}
            </button>
          ) : null}
        </div>
      ) : (
        <div className="target-group-controls">
          <label className="switch-control switch-control--interactive">
            <input
              type="checkbox"
              role="switch"
              aria-label={group.consumers.map((c) => c.agentName).join(", ")}
              checked={group.desired}
              disabled={busy}
              onChange={onToggle}
            />
            <span aria-hidden="true" />
          </label>
          <span className="target-group-state">
            {group.desired
              ? t("library.activation.state.enabled", {
                  state: observedLabel,
                })
              : t("library.activation.state.disabled")}
          </span>
          {group.desired && observedDisplayed(group) && (
            <button
              type="button"
              className="toolbar-button"
              disabled={busy}
              onClick={onRepair}
            >
              {t("inspector.repair")}
            </button>
          )}
        </div>
      )}
    </li>
  );
}

function tPluralConsumerCount(
  count: number,
  t: LocaleContextValue["t"],
): string {
  return count === 1
    ? t("inspector.consumerCount_one", { count })
    : t("inspector.consumerCount_other", { count });
}

function observedDisplayed(group: GlobalTargetGroup): boolean {
  // Repair is only safe for a missing managed entry. Dangling, occupied and
  // target-mismatch observations need their typed conflict flow instead of
  // silently replacing an external entry.
  return group.observedState === "missing";
}

function observedStateLabel(
  group: GlobalTargetGroup,
  t: LocaleContextValue["t"],
): string {
  switch (group.observedState) {
    case "present":
      return t("library.activation.observed.present");
    case "missing":
      return t("library.activation.observed.missing");
    case "dangling":
      return t("library.activation.observed.dangling");
    case "occupied":
      return t("library.activation.observed.occupied");
    case "target_mismatch":
      return t("library.activation.observed.target_mismatch");
    default:
      return t("library.activation.observed.unknown");
  }
}
