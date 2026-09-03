import { useEffect, useState } from "react";

import type {
  CatalogClient,
  EnableResult,
  GlobalTargetGroup,
  GlobalTargetGroupSnapshot,
} from "../../app/catalog-client";
import { LockIcon } from "../../ui/icons";
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
}: {
  ref: React.Ref<HTMLElement | null>;
  dialog: boolean;
  skillId: string;
  client: CatalogClient;
  onOpenAgentManagement: () => void;
}) {
  const { t } = useLocale();
  const [snapshot, setSnapshot] = useState<GlobalTargetGroupSnapshot | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [busyGroupId, setBusyGroupId] = useState<string | null>(null);
  const [lastResult, setLastResult] = useState<EnableResult | null>(null);
  const [lastGroupId, setLastGroupId] = useState<string | null>(null);
  const [sheetInitial, setSheetInitial] = useState<string[] | null>(null);

  async function load() {
    try {
      const next = await client.listTargetGroups(skillId);
      setSnapshot(next);
      setError(null);
    } catch (cause) {
      setError(String(cause));
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
        if (current) setError(String(cause));
      });
    return () => {
      current = false;
    };
  }, [client, skillId]);

  async function onToggle(group: GlobalTargetGroup) {
    if (group.action !== "none") {
      onOpenAgentManagement();
      return;
    }
    setBusyGroupId(group.targetRootId);
    setError(null);
    try {
      const plan = await client.planGlobalLifecycle(
        skillId,
        group.targetRootId,
        group.desired ? "disable" : "enable",
      );
      const cell = plan.cells[0];
      if (cell.eligibility === "conflict") {
        setSheetInitial([group.targetRootId]);
        return;
      }
      const applied = await client.applyGlobalEnable(plan.planToken);
      setLastResult(applied);
      setLastGroupId(group.targetRootId);
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusyGroupId(null);
    }
  }

  async function onRepair(group: GlobalTargetGroup) {
    setBusyGroupId(group.targetRootId);
    setError(null);
    try {
      const plan = await client.planGlobalLifecycle(
        skillId,
        group.targetRootId,
        "repair",
      );
      const applied = await client.applyGlobalEnable(plan.planToken);
      setLastResult(applied);
      setLastGroupId(group.targetRootId);
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusyGroupId(null);
    }
  }

  async function onUndo() {
    if (!lastResult) {
      return;
    }
    setBusyGroupId(lastGroupId);
    try {
      await client.undoGlobalEnable(lastResult.operationId);
      setLastResult(null);
      setLastGroupId(null);
      await load();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusyGroupId(null);
    }
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
        <button
          type="button"
          className="toolbar-button"
          disabled={snapshot === null}
          onClick={() => setSheetInitial([])}
        >
          {t("inspector.enableGlobally")}
        </button>
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
              busy={busyGroupId === group.targetRootId}
              onToggle={() => void onToggle(group)}
              onRepair={() => void onRepair(group)}
              onOpenAgentManagement={onOpenAgentManagement}
            />
          ))}
        </ul>
      )}

      {lastResult !== null && (
        <div className="inspector-result" role="status">
          <p>
            {t("enable.global.resultSummary", {
              succeeded: lastResult.cells.filter(
                (cell) => cell.outcome === "succeeded",
              ).length,
              total: lastResult.cells.length,
            })}
          </p>
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

      {sheetInitial !== null && (
        <GlobalEnableSheet
          client={client}
          skillId={skillId}
          skillName={snapshot?.skillName ?? skillId}
          initialGroupIds={sheetInitial}
          onClose={() => {
            setSheetInitial(null);
            void load();
          }}
        />
      )}
    </aside>
  );
}

function TargetGroupCard({
  group,
  busy,
  onToggle,
  onRepair,
  onOpenAgentManagement,
}: {
  group: GlobalTargetGroup;
  busy: boolean;
  onToggle: () => void;
  onRepair: () => void;
  onOpenAgentManagement: () => void;
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
          (consumer) => consumer.compatibility === "unknown",
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
  return (
    group.observedState === "missing" ||
    group.observedState === "dangling" ||
    group.observedState === "occupied" ||
    group.observedState === "target_mismatch"
  );
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
