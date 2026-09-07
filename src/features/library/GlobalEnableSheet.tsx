import { OperationNotice } from "../../ui/OperationNotice";
import { useEffect, useMemo, useState } from "react";

import type {
  CatalogClient,
  CellResolution,
  EnableCell,
  EnablePlan,
  EnableResult,
  GlobalTargetGroupSnapshot,
} from "../../app/catalog-client";
import { useLocale, type LocaleContextValue } from "../locale/LocaleProvider";
import { commandErrorMessage } from "../locale/messages";
import { useModalFocus } from "../../ui/useModalFocus";

export interface EnableSkillItem {
  id: string;
  name: string;
}

/**
 * Global Enable three-step sheet (spec §7.4): ① Target group selection
 * (default empty, explicit Select all, deduped physical Target count and
 * affected Agent count) → ② Skill × resolved Target preview matrix with the
 * §7.5 per-cell Conflict decisions → ③ Result with one `Undo this
 * operation`. The Core owns the plan; this surface never composes per-Agent
 * commands.
 */
export function GlobalEnableSheet({
  client,
  skills,
  skillId,
  skillName,
  initialGroupIds = [],
  opener,
  onAdoptExisting,
  onClose,
  onCatalogChanged,
}: {
  client: CatalogClient;
  skills?: EnableSkillItem[];
  skillId?: string;
  skillName?: string;
  initialGroupIds?: string[];
  opener?: HTMLElement | null;
  onAdoptExisting?: (cell: EnableCell) => void;
  onClose: () => void;
  onCatalogChanged?: () => Promise<void>;
}) {
  const { t, tPlural } = useLocale();
  const normalizedSkills = useMemo<EnableSkillItem[]>(() => {
    if (skills && skills.length > 0) return skills;
    if (skillId) return [{ id: skillId, name: skillName ?? skillId }];
    return [];
  }, [skills, skillId, skillName]);
  const isBatch = normalizedSkills.length > 1;
  const primarySkillId = normalizedSkills[0]?.id ?? "";
  const skillIds = useMemo(
    () => normalizedSkills.map((s) => s.id),
    [normalizedSkills],
  );
  const [groupsSnapshot, setGroupsSnapshot] =
    useState<GlobalTargetGroupSnapshot | null>(null);
  const [selected, setSelected] = useState<string[]>(initialGroupIds);
  const [resolutions, setResolutions] = useState<Map<string, CellResolution>>(
    new Map(),
  );
  const [plan, setPlan] = useState<EnablePlan | null>(null);
  const [result, setResult] = useState<EnableResult | null>(null);
  const [shownStep, setShownStep] = useState<"targets" | "preview" | "result">(
    "targets",
  );
  const [error, setError] = useState<string | null>(null);
  const [activity, setActivity] = useState<
    "idle" | "applying" | "undoing" | "finalizing"
  >("idle");

  const step = shownStep;
  const preparing = step === "targets" && groupsSnapshot === null;
  const planning = step === "preview" && plan === null;
  const busy = preparing || planning || activity !== "idle";
  const modalRef = useModalFocus<HTMLElement>({
    opener,
    busy,
    focusKey: `${shownStep}:${groupsSnapshot !== null}:${plan !== null}:${result !== null}`,
    onClose: onCloseWithFinalize,
  });
  async function refreshGroups() {
    try {
      if (!primarySkillId) return;
      setGroupsSnapshot(await client.listTargetGroups(primarySkillId));
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    }
  }

  useEffect(() => {
    if (!primarySkillId) return;
    let current = true;
    client
      .listTargetGroups(primarySkillId)
      .then((next) => {
        if (!current) return;
        setGroupsSnapshot(next);
        setError(null);
      })
      .catch((cause) => {
        if (current) setError(commandErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, primarySkillId, t]);

  // Re-plan whenever the resolution set changes so the preview matrix always
  // reflects the user's latest per-cell decisions; only after Continue.
  useEffect(() => {
    if (
      shownStep !== "preview" ||
      selected.length === 0 ||
      groupsSnapshot === null ||
      skillIds.length === 0
    ) {
      return;
    }
    let current = true;
    client
      .planGlobalEnable(
        skillIds,
        selected,
        [...resolutions.entries()].map(([cellKey, resolution]) => ({
          cellKey,
          resolution,
        })),
      )
      .then((next) => {
        if (!current) return;
        setPlan(next);
        setError(null);
      })
      .catch((cause) => {
        if (current) setError(commandErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, skillIds, selected, resolutions, groupsSnapshot, shownStep, t]);

  const applyableCells = useMemo(
    () =>
      (plan?.cells ?? []).filter(
        (cell) =>
          cell.eligibility === "ready" ||
          (cell.eligibility === "conflict" &&
            cell.resolution !== "skip" &&
            cell.resolution !== "adopt"),
      ),
    [plan],
  );

  const chosenGroups = (groupsSnapshot?.groups ?? []).filter((group) =>
    selected.includes(group.targetRootId),
  );
  const affectedAgentIds = new Set(
    chosenGroups.flatMap((group) => group.consumers.map((c) => c.agentId)),
  );

  async function onApply() {
    if (!plan || activity !== "idle") {
      return;
    }
    setActivity("applying");
    setError(null);
    try {
      const applied = await client.applyGlobalEnable(plan.planToken);
      setResult(applied);
      setShownStep("result");
      await onCatalogChanged?.();
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setActivity("idle");
    }
  }

  async function onUndo() {
    if (!result || activity !== "idle") {
      return;
    }
    setActivity("undoing");
    setError(null);
    try {
      await client.undoGlobalEnable(result.operationId);
      setResult(null);
      setPlan(null);
      setShownStep("targets");
      await refreshGroups();
      await onCatalogChanged?.();
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setActivity("idle");
    }
  }

  async function onCloseWithFinalize() {
    if (busy) return;
    if (result) {
      setActivity("finalizing");
      try {
        await client.finalizeGlobalEnable(result.operationId);
      } catch {
        // Finalization is best-effort here: the result window closes on
        // restart as well (spec §4.9).
      } finally {
        setActivity("idle");
      }
    }
    onClose();
  }

  return (
    <div
      className="activation-sheet-backdrop"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target && !busy) {
          void onCloseWithFinalize();
        }
      }}
    >
      <section
        ref={modalRef}
        className="activation-sheet enable-sheet"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        aria-label={
          isBatch
            ? tPlural("enable.global.dialogLabelBatch", normalizedSkills.length)
            : t("enable.global.dialogLabel", {
                skill: normalizedSkills[0]?.name ?? "",
              })
        }
      >
        <header className="enable-sheet-heading">
          <h2>
            {isBatch
              ? tPlural(
                  "enable.global.dialogLabelBatch",
                  normalizedSkills.length,
                )
              : t("enable.global.dialogLabel", {
                  skill: normalizedSkills[0]?.name ?? "",
                })}
          </h2>
        </header>
        <ol
          className="import-progress"
          aria-label={t("enable.global.progressLabel")}
        >
          {(["targets", "preview", "result"] as const).map((s) => (
            <li key={s} aria-current={step === s ? "step" : undefined}>
              {t(
                s === "targets"
                  ? "enable.global.stepTargetGroups"
                  : s === "preview"
                    ? "enable.global.stepPreviewMatrix"
                    : "enable.global.stepResult",
              )}
            </li>
          ))}
        </ol>

        <OperationNotice busy={busy} />
        <div className="enable-sheet-body">
          {step === "targets" && (
            <TargetGroupStep
              snapshot={groupsSnapshot}
              selected={selected}
              activity={busy}
              onToggle={(groupId) => {
                setSelected((current) =>
                  current.includes(groupId)
                    ? current.filter((id) => id !== groupId)
                    : [...current, groupId],
                );
              }}
              onSelectAll={() =>
                setSelected(
                  (groupsSnapshot?.groups ?? []).map(
                    (group) => group.targetRootId,
                  ),
                )
              }
              affectedAgentCount={affectedAgentIds.size}
            />
          )}

          {step === "preview" && plan !== null && (
            <PreviewMatrixStep
              plan={plan}
              resolutions={resolutions}
              isBatch={isBatch}
              onAdoptExisting={onAdoptExisting}
              onResolutionChange={(cellKey, resolution) => {
                setResolutions((current) => {
                  const next = new Map(current);
                  next.set(cellKey, resolution);
                  if (resolution === "replace" || resolution === "switch") {
                    const changedCell = plan.cells.find(
                      (c) => c.cellKey === cellKey,
                    );
                    if (changedCell) {
                      for (const other of plan.cells) {
                        if (
                          other.cellKey !== cellKey &&
                          other.targetRootId === changedCell.targetRootId &&
                          other.entryPath === changedCell.entryPath
                        ) {
                          next.set(other.cellKey, "skip");
                        }
                      }
                    }
                  }
                  return next;
                });
              }}
            />
          )}

          {step === "result" && result !== null && (
            <ResultStep
              result={result}
              busy={busy}
              onUndo={() => void onUndo()}
            />
          )}

          {error !== null && (
            <p className="enable-sheet-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <div className="import-actions enable-sheet-actions">
          {step !== "result" && (
            <button
              type="button"
              className="toolbar-button"
              disabled={busy}
              onClick={() => void onCloseWithFinalize()}
            >
              {t("sourceGroup.cancel")}
            </button>
          )}
          {step === "targets" && (
            <button
              type="button"
              className="toolbar-button primary"
              disabled={
                selected.length === 0 || busy || groupsSnapshot === null
              }
              onClick={() => {
                setPlan(null);
                setResolutions(new Map());
                setShownStep("preview");
              }}
            >
              {t("enable.global.continue")}
            </button>
          )}
          {step === "preview" && (
            <>
              <button
                type="button"
                className="toolbar-button"
                disabled={busy}
                onClick={() => setShownStep("targets")}
              >
                {t("enable.global.back")}
              </button>
              <button
                type="button"
                className="toolbar-button primary"
                disabled={applyableCells.length === 0 || busy}
                onClick={() => void onApply()}
              >
                {t("enable.global.applyPlan")}
              </button>
            </>
          )}
          {step === "result" && (
            <button
              type="button"
              className="toolbar-button primary"
              disabled={busy}
              onClick={() => void onCloseWithFinalize()}
            >
              {t("enable.global.close")}
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

function TargetGroupStep({
  snapshot,
  selected,
  activity,
  onToggle,
  onSelectAll,
  affectedAgentCount,
}: {
  snapshot: GlobalTargetGroupSnapshot | null;
  selected: string[];
  activity: boolean;
  onToggle: (groupId: string) => void;
  onSelectAll: () => void;
  affectedAgentCount: number;
}) {
  const { t, tPlural } = useLocale();
  if (snapshot === null) {
    return (
      <p className="enable-sheet-empty">
        {activity
          ? t("library.activation.preparing")
          : t("enable.global.noGroups")}
      </p>
    );
  }
  if (snapshot.groups.length === 0) {
    return <p className="enable-sheet-empty">{t("enable.global.noGroups")}</p>;
  }
  return (
    <div className="enable-group-selection">
      <p className="enable-selection-summary">
        {selected.length === 0
          ? t("enable.global.selectedNone")
          : tPlural("enable.global.selectedGroups", selected.length)}
        {" · "}
        {tPlural("enable.global.affectedAgents", affectedAgentCount)}
        {" · "}
        {tPlural("enable.global.physicalTargets", new Set(selected).size)}
      </p>
      <ul className="enable-group-list">
        {snapshot.groups.map((group) => (
          <li key={group.targetRootId}>
            <label>
              <input
                type="checkbox"
                checked={selected.includes(group.targetRootId)}
                disabled={group.action !== "none" || activity}
                onChange={() => onToggle(group.targetRootId)}
              />
              <span className="enable-group-name">
                {group.consumers.map((c) => c.agentName).join(", ")}
              </span>
              <small className="enable-group-path">
                {group.configuredPath}
              </small>
              {group.action === "open_agent_management" && (
                <em>{t("enable.global.mismatchNote")}</em>
              )}
            </label>
          </li>
        ))}
      </ul>
      <div className="import-actions">
        <button
          type="button"
          className="toolbar-button"
          disabled={activity}
          onClick={onSelectAll}
        >
          {t("enable.global.selectAll")}
        </button>
      </div>
    </div>
  );
}

function PreviewMatrixStep({
  plan,
  resolutions,
  isBatch,
  onAdoptExisting,
  onResolutionChange,
}: {
  plan: EnablePlan;
  resolutions: Map<string, CellResolution>;
  isBatch: boolean;
  onAdoptExisting?: (cell: EnableCell) => void;
  onResolutionChange: (cellKey: string, resolution: CellResolution) => void;
}) {
  const { t } = useLocale();
  return (
    <div className="enable-preview-matrix" role="table">
      <div className="enable-matrix-head" role="row">
        <span role="columnheader">{t("enable.global.stepPreviewMatrix")}</span>
      </div>
      {plan.cells.map((cell) => (
        <PreviewCell
          key={cell.cellKey}
          cell={cell}
          resolution={resolutions.get(cell.cellKey) ?? cell.resolution}
          isBatch={isBatch}
          onAdoptExisting={onAdoptExisting}
          onResolutionChange={onResolutionChange}
        />
      ))}
      <p className="enable-sheet-hint">{t("enable.global.conflictHint")}</p>
    </div>
  );
}

function PreviewCell({
  cell,
  resolution,
  isBatch,
  onAdoptExisting,
  onResolutionChange,
}: {
  cell: EnableCell;
  resolution: CellResolution;
  isBatch: boolean;
  onAdoptExisting?: (cell: EnableCell) => void;
  onResolutionChange: (cellKey: string, resolution: CellResolution) => void;
}) {
  const { t } = useLocale();
  const eligibilityLabel =
    cell.eligibility === "ready"
      ? t("enable.global.cellReady")
      : cell.eligibility === "no_op"
        ? t("enable.global.cellNoOp")
        : cell.eligibility === "conflict"
          ? t("enable.global.cellConflict")
          : cell.eligibility === "blocked"
            ? t("enable.global.cellBlocked")
            : t("enable.global.cellSkip");
  const blockedLabel = blockedReasonLabel(cell, t);
  return (
    <div className="enable-matrix-row" role="row">
      <span className="enable-matrix-cell" role="cell">
        {cell.skillName} → {cell.targetPath}
      </span>
      <span
        className={`enable-matrix-eligibility eligibility-${cell.eligibility}`}
        role="cell"
      >
        {eligibilityLabel}
      </span>
      {cell.eligibility === "conflict" && (
        <>
          <select
            className="enable-resolution-select"
            aria-label={cell.cellKey}
            value={resolution}
            onChange={(event) =>
              onResolutionChange(
                cell.cellKey,
                event.currentTarget.value as CellResolution,
              )
            }
          >
            {isBatch ? (
              <>
                {typeof cell.occupier === "object" &&
                  "managed" in cell.occupier && (
                    <option value="switch">
                      {t("enable.global.resolutionSwitch")}
                    </option>
                  )}
                <option value="replace">
                  {t("enable.global.resolutionReplaceBatch")}
                </option>
                <option value="skip">
                  {t("enable.global.resolutionSkipBatch")}
                </option>
              </>
            ) : (
              <>
                <option value="switch">
                  {t("enable.global.resolutionSwitch")}
                </option>
                <option value="replace">
                  {t("enable.global.resolutionReplace")}
                </option>
                <option value="adopt">
                  {t("enable.global.resolutionAdopt")}
                </option>
                <option value="skip">
                  {t("enable.global.resolutionCancel")}
                </option>
              </>
            )}
          </select>
          {!isBatch && resolution === "adopt" && onAdoptExisting ? (
            <button
              type="button"
              className="toolbar-button"
              onClick={() => onAdoptExisting(cell)}
            >
              {t("enable.global.openAdopt")}
            </button>
          ) : null}
        </>
      )}
      {blockedLabel !== null && (
        <small className="enable-matrix-note">{blockedLabel}</small>
      )}
      {cell.detail !== null && (
        <small className="enable-matrix-note">{cell.detail}</small>
      )}
      {cell.occExactDirect && (
        <small className="enable-matrix-note">
          {t("enable.global.exactDirectNote")}
        </small>
      )}
      {cell.destructive && (
        <small className="enable-matrix-note">
          {t("enable.global.destructive", {
            files: cell.destructive.files,
            directories: cell.destructive.directories,
          })}
        </small>
      )}
      <small className="enable-matrix-agents">
        {cell.affectedAgentNames.join(", ")}
      </small>
    </div>
  );
}

function blockedReasonLabel(
  cell: EnableCell,
  t: LocaleContextValue["t"],
): string | null {
  switch (cell.blockedReason) {
    case "target_absent":
      return t("enable.global.blockedTargetAbsent");
    case "target_unavailable":
      return t("enable.global.blockedTargetUnavailable");
    case "source_snapshot_mismatch":
      return t("enable.global.blockedSourceSnapshotMismatch");
    case "tombstoned_member":
      return t("enable.global.blockedTombstonedMember");
    case "entity_broken":
      return t("enable.global.blockedEntityBroken");
    case "entry_occupied":
      return t("enable.global.blockedEntryOccupied");
    default:
      return null;
  }
}

function ResultStep({
  result,
  busy,
  onUndo,
}: {
  result: EnableResult;
  busy: boolean;
  onUndo: () => void;
}) {
  const { t } = useLocale();
  const succeeded = result.cells.filter(
    (cell) => cell.outcome === "succeeded",
  ).length;
  return (
    <div className="enable-result">
      <p>
        {t("enable.global.resultSummary", {
          succeeded,
          total: result.cells.length,
        })}
      </p>
      <ul>
        {result.cells.map((cell) => (
          <li key={cell.cellKey}>
            <span className={`enable-outcome outcome-${cell.outcome}`}>
              {outcomeLabel(cell.outcome, t)}
            </span>
            {" — "}
            {cell.cellKey}
            {cell.diagnostic !== null && (
              <small className="enable-matrix-note"> {cell.diagnostic}</small>
            )}
          </li>
        ))}
      </ul>
      <button
        type="button"
        className="toolbar-button"
        disabled={busy}
        onClick={onUndo}
      >
        {t("enable.global.undoOperation")}
      </button>
    </div>
  );
}

function outcomeLabel(
  outcome: EnableResult["cells"][number]["outcome"],
  t: LocaleContextValue["t"],
): string {
  switch (outcome) {
    case "succeeded":
      return t("enable.global.outcomeSucceeded");
    case "no_op":
      return t("enable.global.outcomeNoOp");
    case "skipped":
      return t("enable.global.outcomeSkipped");
    case "failed":
      return t("enable.global.outcomeFailed");
    case "not_attempted":
      return t("enable.global.outcomeNotAttempted");
  }
}
