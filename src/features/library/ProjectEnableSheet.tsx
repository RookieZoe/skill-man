import { OperationNotice } from "../../ui/OperationNotice";
import { useEffect, useMemo, useState } from "react";

import type {
  AgentConfiguration,
  CatalogClient,
  CellResolution,
  EnableCell,
  EnablePlan,
  EnableResult,
  RecentProjectFolder,
} from "../../app/catalog-client";
import type { EnableSkillItem } from "./GlobalEnableSheet";
import { useLocale } from "../locale/LocaleProvider";
import { commandErrorMessage, type MessageKey } from "../locale/messages";
import { useModalFocus } from "../../ui/useModalFocus";

/**
 * Project Enable four-step sheet (spec §7.4; ADR-0015; ADR-0019; ADR-0021; #89):
 * ① Select project folder (MRU candidates + manual path; MRU candidate only,
 *    not auto-selected)
 * ② Select Agent(s) with safe `project_skills_dir` (disabled if none, with
 *    action to Agent Management; selection starts empty, draft cleared on close)
 * ③ Preview (hop evidence, resolved-target group deduplication to "1 physical
 *    write", occupancy handling, per-cell destructive ack for real directories,
 *    no Replace-all)
 * ④ Result (warning "no Project record created", cell outcomes, Undo window
 *    open until sheet closed; closing finalizes).
 */
export function ProjectEnableSheet({
  client,
  skills,
  skillId,
  skillName,
  opener,
  onClose,
  onOpenAgentManagement,
}: {
  client: CatalogClient;
  skills?: EnableSkillItem[];
  skillId?: string;
  skillName?: string;
  opener?: HTMLElement | null;
  onClose: () => void;
  onOpenAgentManagement?: () => void;
}) {
  const { t, tPlural } = useLocale();
  const normalizedSkills = useMemo<EnableSkillItem[]>(() => {
    if (skills && skills.length > 0) return skills;
    if (skillId) return [{ id: skillId, name: skillName ?? skillId }];
    return [];
  }, [skills, skillId, skillName]);
  const isBatch = normalizedSkills.length > 1;
  const skillIds = useMemo(
    () => normalizedSkills.map((s) => s.id),
    [normalizedSkills],
  );

  const [step, setStep] = useState<"folder" | "agents" | "preview" | "result">(
    "folder",
  );
  const [folder, setFolder] = useState("");
  const [recentFolders, setRecentFolders] = useState<RecentProjectFolder[]>([]);
  const [agents, setAgents] = useState<AgentConfiguration[]>([]);
  const [selectedAgentIds, setSelectedAgentIds] = useState<string[]>([]);
  const [plan, setPlan] = useState<EnablePlan | null>(null);
  const [resolutions, setResolutions] = useState<Map<string, CellResolution>>(
    new Map(),
  );
  const [destructiveAcks, setDestructiveAcks] = useState<Set<string>>(
    new Set(),
  );
  const [result, setResult] = useState<EnableResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const planning = step === "preview" && plan === null;
  const busy = planning || submitting;
  const modalRef = useModalFocus<HTMLElement>({
    opener,
    busy,
    focusKey: `${step}:${agents.length}:${plan !== null}:${result !== null}`,
    onClose: onCloseWithFinalize,
  });

  // Load recent folders on mount
  useEffect(() => {
    let current = true;
    client
      .listRecentProjectFolders()
      .then((folders) => {
        if (current) setRecentFolders(folders);
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [client]);

  // Load agent configurations when moving to agents step
  useEffect(() => {
    if (step !== "agents") return;
    let current = true;
    client
      .getAgentManagementSnapshot()
      .then((snapshot) => {
        if (current) setAgents(snapshot.configurations);
      })
      .catch((cause) => {
        if (current) setError(commandErrorMessage(cause, t));
      });
    return () => {
      current = false;
    };
  }, [client, step, t]);

  // Plan project enable whenever preview step is active and inputs change
  useEffect(() => {
    if (
      step !== "preview" ||
      selectedAgentIds.length === 0 ||
      !folder.trim() ||
      skillIds.length === 0
    ) {
      return;
    }
    let current = true;
    client
      .planProjectEnable(
        skillIds,
        folder.trim(),
        selectedAgentIds,
        [...resolutions.entries()].map(([cellKey, resolution]) => ({
          cellKey,
          resolution,
        })),
      )
      .then((nextPlan) => {
        if (!current) return;
        setPlan(nextPlan);
        setError(null);
      })
      .catch((cause) => {
        if (current) {
          setError(commandErrorMessage(cause, t));
        }
      });
    return () => {
      current = false;
    };
  }, [client, skillIds, folder, selectedAgentIds, resolutions, step, t]);

  const applyableCells = useMemo(
    () =>
      (plan?.cells ?? []).filter(
        (cell) =>
          cell.eligibility === "ready" ||
          (cell.eligibility === "conflict" && cell.resolution === "replace"),
      ),
    [plan],
  );

  // Check whether any real directory replace is missing its destructive ack
  const missingDestructiveAck = useMemo(() => {
    for (const cell of plan?.cells ?? []) {
      if (
        cell.eligibility === "conflict" &&
        cell.resolution === "replace" &&
        cell.destructive !== null &&
        !destructiveAcks.has(cell.cellKey)
      ) {
        return true;
      }
    }
    return false;
  }, [plan, destructiveAcks]);

  async function onApply() {
    if (!plan || missingDestructiveAck) return;
    setSubmitting(true);
    setError(null);
    try {
      const enableResult = await client.applyProjectEnable(plan.planToken);
      setResult(enableResult);
      setStep("result");
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setSubmitting(false);
    }
  }

  async function onUndo() {
    if (!result) return;
    setSubmitting(true);
    setError(null);
    try {
      await client.undoProjectEnable(result.operationId);
      setResult(null);
      setPlan(null);
      setStep("folder");
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    } finally {
      setSubmitting(false);
    }
  }

  async function onCloseWithFinalize() {
    if (busy) return;
    if (result) {
      setSubmitting(true);
      try {
        await client.finalizeProjectEnable(result.operationId);
      } catch {
        // Finalization is best effort
      } finally {
        setSubmitting(false);
      }
    }
    onClose();
  }

  async function onClearRecent() {
    try {
      await client.clearRecentProjectFolders();
      setRecentFolders([]);
    } catch (cause) {
      setError(commandErrorMessage(cause, t));
    }
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
        className="activation-sheet enable-sheet project-enable-sheet"
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        aria-label={
          isBatch
            ? tPlural(
                "enable.project.dialogLabelBatch",
                normalizedSkills.length,
              )
            : t("enable.project.dialogLabel", {
                skill: normalizedSkills[0]?.name ?? "",
              })
        }
      >
        <header className="enable-sheet-heading">
          <h2>
            {isBatch
              ? tPlural(
                  "enable.project.dialogLabelBatch",
                  normalizedSkills.length,
                )
              : t("enable.project.dialogLabel", {
                  skill: normalizedSkills[0]?.name ?? "",
                })}
          </h2>
        </header>
        <ol
          className="import-progress"
          aria-label={t("enable.project.progressLabel")}
        >
          {(["folder", "agents", "preview", "result"] as const).map((s) => (
            <li key={s} aria-current={step === s ? "step" : undefined}>
              {t(`enable.project.step${capitalize(s)}` as MessageKey)}
            </li>
          ))}
        </ol>

        <div className="enable-sheet-body">
          {step === "folder" && (
            <FolderStep
              folder={folder}
              recentFolders={recentFolders}
              onSelectFolder={(selected) => setFolder(selected)}
              onFolderChange={(next) => setFolder(next)}
              onClearRecent={onClearRecent}
            />
          )}

          {step === "agents" && (
            <AgentSelectionStep
              agents={agents}
              selectedIds={selectedAgentIds}
              onToggleAgent={(id) =>
                setSelectedAgentIds((curr) =>
                  curr.includes(id)
                    ? curr.filter((item) => item !== id)
                    : [...curr, id],
                )
              }
              onSelectAll={() =>
                setSelectedAgentIds(
                  agents
                    .filter((agent) => Boolean(agent.projectSkillsDir))
                    .map((agent) => agent.agentId),
                )
              }
              onOpenAgentManagement={onOpenAgentManagement}
            />
          )}

          {step === "preview" && plan !== null && (
            <ProjectPreviewStep
              plan={plan}
              resolutions={resolutions}
              destructiveAcks={destructiveAcks}
              onResolutionChange={(cellKey, resolution) => {
                setResolutions((curr) => {
                  const next = new Map(curr);
                  next.set(cellKey, resolution);
                  if (resolution === "replace") {
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
              onToggleDestructiveAck={(cellKey) => {
                setDestructiveAcks((curr) => {
                  const next = new Set(curr);
                  if (next.has(cellKey)) {
                    next.delete(cellKey);
                  } else {
                    next.add(cellKey);
                  }
                  return next;
                });
              }}
            />
          )}

          {step === "result" && result !== null && (
            <ProjectResultStep result={result} onUndo={() => void onUndo()} />
          )}

          {error !== null && (
            <p className="enable-sheet-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <OperationNotice busy={busy} />
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
          {step === "folder" && (
            <button
              type="button"
              className="toolbar-button primary"
              disabled={folder.trim().length === 0}
              onClick={() => setStep("agents")}
            >
              {t("enable.project.continue")}
            </button>
          )}

          {step === "agents" && (
            <>
              <button
                type="button"
                className="toolbar-button"
                onClick={() => setStep("folder")}
              >
                {t("enable.project.back")}
              </button>
              <button
                type="button"
                className="toolbar-button primary"
                disabled={selectedAgentIds.length === 0}
                onClick={() => {
                  setPlan(null);
                  setResolutions(new Map());
                  setDestructiveAcks(new Set());
                  setStep("preview");
                }}
              >
                {t("enable.project.continue")}
              </button>
            </>
          )}

          {step === "preview" && (
            <>
              <button
                type="button"
                className="toolbar-button"
                disabled={busy}
                onClick={() => setStep("agents")}
              >
                {t("enable.project.back")}
              </button>
              <button
                type="button"
                className="toolbar-button primary"
                disabled={
                  applyableCells.length === 0 || missingDestructiveAck || busy
                }
                onClick={() => void onApply()}
              >
                {t("enable.project.applyPlan")}
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
              {t("enable.project.close")}
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function FolderStep({
  folder,
  recentFolders,
  onSelectFolder,
  onFolderChange,
  onClearRecent,
}: {
  folder: string;
  recentFolders: RecentProjectFolder[];
  onSelectFolder: (path: string) => void;
  onFolderChange: (path: string) => void;
  onClearRecent: () => void;
}) {
  const { t } = useLocale();

  return (
    <div className="project-folder-selection">
      {recentFolders.length > 0 && (
        <section className="recent-folders-section">
          <div className="recent-folders-header">
            <h3>{t("enable.project.recentFolders")}</h3>
            <button
              type="button"
              className="toolbar-button"
              onClick={onClearRecent}
            >
              {t("enable.project.clearRecentFolders")}
            </button>
          </div>
          <ul className="recent-folder-list">
            {recentFolders.map((item) => (
              <li key={item.canonicalPathKey}>
                <button
                  type="button"
                  className={`recent-folder-item ${folder === item.canonicalPath ? "selected" : ""}`}
                  onClick={() => onSelectFolder(item.canonicalPath)}
                >
                  <span className="recent-folder-path">
                    {item.canonicalPath}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <div className="folder-input-section">
        <label htmlFor="project-folder-input">
          {t("enable.project.folderLabel")}
        </label>
        <div className="folder-input-row">
          <input
            id="project-folder-input"
            type="text"
            className="text-input"
            placeholder={t("enable.project.folderPlaceholder")}
            value={folder}
            onChange={(e) => onFolderChange(e.currentTarget.value)}
          />
          <button
            type="button"
            className="toolbar-button"
            onClick={async () => {
              const picked = await browseProjectFolder();
              if (picked) {
                onFolderChange(picked);
              }
            }}
          >
            {t("enable.project.browse")}
          </button>
        </div>
      </div>
    </div>
  );
}

async function browseProjectFolder(): Promise<string | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selection = await open({ directory: true, multiple: false });
    return typeof selection === "string" ? selection : null;
  } catch {
    return null;
  }
}
function AgentSelectionStep({
  agents,
  selectedIds,
  onToggleAgent,
  onSelectAll,
  onOpenAgentManagement,
}: {
  agents: AgentConfiguration[];
  selectedIds: string[];
  onToggleAgent: (id: string) => void;
  onSelectAll: () => void;
  onOpenAgentManagement?: () => void;
}) {
  const { t, tPlural } = useLocale();

  const configurableAgents = agents.filter((agent) =>
    Boolean(agent.projectSkillsDir),
  );

  return (
    <div className="project-agent-selection">
      <p className="enable-selection-summary">
        {selectedIds.length === 0
          ? t("enable.project.selectedNone")
          : tPlural("enable.project.selectedAgents", selectedIds.length)}
      </p>

      {agents.length === 0 ? (
        <p className="enable-sheet-empty">{t("enable.project.noAgents")}</p>
      ) : (
        <ul className="project-agent-list">
          {agents.map((agent) => {
            const hasProjectDir = Boolean(agent.projectSkillsDir);
            const isSelected = selectedIds.includes(agent.agentId);

            return (
              <li
                key={agent.agentId}
                className={`project-agent-row ${!hasProjectDir ? "disabled" : ""}`}
              >
                <label>
                  <input
                    type="checkbox"
                    checked={isSelected}
                    disabled={!hasProjectDir}
                    onChange={() => onToggleAgent(agent.agentId)}
                  />
                  <div className="project-agent-info">
                    <span className="agent-name">{agent.name}</span>
                    {hasProjectDir ? (
                      <small className="project-skills-dir">
                        {agent.projectSkillsDir}
                      </small>
                    ) : (
                      <span className="no-dir-warning">
                        {t("enable.project.agentDisabledNoDir")}
                      </span>
                    )}
                  </div>
                </label>
                {!hasProjectDir && onOpenAgentManagement && (
                  <button
                    type="button"
                    className="toolbar-button agent-management-link"
                    onClick={onOpenAgentManagement}
                  >
                    {t("enable.project.openAgentManagement")}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {configurableAgents.length > 0 && (
        <div className="agent-selection-actions">
          <button
            type="button"
            className="toolbar-button"
            onClick={onSelectAll}
          >
            {t("enable.project.selectAll")}
          </button>
        </div>
      )}
    </div>
  );
}

function ProjectPreviewStep({
  plan,
  resolutions,
  destructiveAcks,
  onResolutionChange,
  onToggleDestructiveAck,
}: {
  plan: EnablePlan;
  resolutions: Map<string, CellResolution>;
  destructiveAcks: Set<string>;
  onResolutionChange: (cellKey: string, resolution: CellResolution) => void;
  onToggleDestructiveAck: (cellKey: string) => void;
}) {
  const { t } = useLocale();

  return (
    <div className="project-preview-matrix">
      {plan.cells.map((cell) => (
        <ProjectPreviewCell
          key={cell.cellKey}
          cell={cell}
          resolution={resolutions.get(cell.cellKey) ?? cell.resolution}
          hasDestructiveAck={destructiveAcks.has(cell.cellKey)}
          onResolutionChange={onResolutionChange}
          onToggleDestructiveAck={onToggleDestructiveAck}
        />
      ))}
      <p className="enable-sheet-hint">{t("enable.global.conflictHint")}</p>
    </div>
  );
}

function ProjectPreviewCell({
  cell,
  resolution,
  hasDestructiveAck,
  onResolutionChange,
  onToggleDestructiveAck,
}: {
  cell: EnableCell;
  resolution: CellResolution;
  hasDestructiveAck: boolean;
  onResolutionChange: (cellKey: string, resolution: CellResolution) => void;
  onToggleDestructiveAck: (cellKey: string) => void;
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

  const blockedReasonText = blockedReasonLabel(cell, t);

  return (
    <div className="project-cell-card">
      <div className="project-cell-header">
        <div className="resolved-group-info">
          <h4>
            {cell.skillName} → {t("enable.project.resolvedGroupDisclosure")}
          </h4>
          <p className="one-physical-write">
            {t("enable.project.onePhysicalWrite", {
              count: cell.affectedAgentIds.length,
            })}
            {" ("}
            {cell.affectedAgentNames.join(", ")}
            {")"}
          </p>
        </div>
        <span
          className={`enable-matrix-eligibility eligibility-${cell.eligibility}`}
        >
          {eligibilityLabel}
        </span>
      </div>

      <div className="project-evidence-box">
        <div className="evidence-line">
          {t("enable.project.resolvedContainerLabel", {
            path: cell.targetPath,
          })}
        </div>

        {cell.hopEvidence.map((hop) => (
          <div key={hop.agentId} className="agent-hop-evidence">
            <div className="evidence-line">
              {t("enable.project.configuredPathLabel", {
                path: hop.configuredRelativePath,
              })}
            </div>
            {hop.hops.length > 0 && (
              <div className="hops-list">
                <span>{t("enable.project.hopEvidenceLabel")}</span>
                <ul>
                  {hop.hops.map((h, i) => (
                    <li key={i}>
                      <code>{h.path}</code>
                      {h.target && (
                        <>
                          {" "}
                          → <code>{h.target}</code>
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        ))}

        {cell.createSteps.length > 0 && (
          <div className="create-steps-box">
            <span>{t("enable.project.createStepsLabel")}</span>{" "}
            <code>{cell.createSteps.join(", ")}</code>
          </div>
        )}
      </div>

      {cell.eligibility === "conflict" && (
        <div className="project-conflict-resolution">
          <select
            className="enable-resolution-select"
            value={resolution}
            onChange={(e) =>
              onResolutionChange(
                cell.cellKey,
                e.currentTarget.value as CellResolution,
              )
            }
          >
            <option value="replace">
              {t("enable.global.resolutionReplace")}
            </option>
            <option value="skip">{t("enable.global.resolutionCancel")}</option>
          </select>

          {cell.destructive && (
            <div className="destructive-warning-box">
              <small className="destructive-count">
                {t("enable.global.destructive", {
                  files: cell.destructive.files,
                  directories: cell.destructive.directories,
                })}
              </small>
              {resolution === "replace" && (
                <label className="destructive-ack-checkbox">
                  <input
                    type="checkbox"
                    checked={hasDestructiveAck}
                    onChange={() => onToggleDestructiveAck(cell.cellKey)}
                  />
                  <span>{t("enable.project.destructiveAck")}</span>
                </label>
              )}
            </div>
          )}
        </div>
      )}

      {cell.detail !== null && (
        <p className="enable-matrix-note error-note">{cell.detail}</p>
      )}
      {blockedReasonText && (
        <p className="enable-matrix-note error-note">{blockedReasonText}</p>
      )}
    </div>
  );
}

function ProjectResultStep({
  result,
  onUndo,
}: {
  result: EnableResult;
  onUndo: () => void;
}) {
  const { t } = useLocale();

  const succeededCount = result.cells.filter(
    (cell) => cell.outcome === "succeeded",
  ).length;

  return (
    <div className="project-result-panel">
      <div className="project-warning-banner" role="alert">
        <strong>{t("enable.project.noProjectRecordWarning")}</strong>
      </div>

      <p className="result-summary-text">
        {t("enable.project.resultSummary", {
          succeeded: succeededCount,
          total: result.cells.length,
        })}
      </p>

      <ul className="result-cell-list">
        {result.cells.map((c) => (
          <li key={c.cellKey} className="result-cell-item">
            <span className="result-cell-key">{c.cellKey}</span>
            <span className={`result-outcome outcome-${c.outcome}`}>
              {outcomeLabel(c.outcome, t)}
            </span>
          </li>
        ))}
      </ul>

      <div className="result-undo-section">
        <button type="button" className="toolbar-button" onClick={onUndo}>
          {t("enable.project.undoOperation")}
        </button>
      </div>
    </div>
  );
}

function blockedReasonLabel(
  cell: EnableCell,
  t: (key: MessageKey, params?: Record<string, string | number>) => string,
): string | null {
  switch (cell.blockedReason) {
    case "target_absent":
      return t("enable.global.blockedTargetAbsent");
    case "target_unavailable":
      return t("enable.project.blockedUnreadable");
    case "source_snapshot_mismatch":
      return t("enable.global.blockedSourceSnapshotMismatch");
    case "tombstoned_member":
      return t("enable.global.blockedTombstonedMember");
    case "entity_broken":
      return t("enable.global.blockedEntityBroken");
    case "entry_occupied":
      return t("enable.global.blockedEntryOccupied");
    case "outside_project_root":
      return t("enable.project.blockedOutside");
    case "symlink_cycle":
      return t("enable.project.blockedCycle");
    case "hop_limit_exceeded":
      return t("enable.project.blockedHopLimit");
    case "target_not_directory":
      return t("enable.project.blockedTargetNotDir");
    default:
      return null;
  }
}

function outcomeLabel(outcome: string, t: (key: MessageKey) => string): string {
  switch (outcome) {
    case "succeeded":
      return t("enable.global.outcomeSucceeded");
    case "no_op":
      return t("enable.global.outcomeNoOp");
    case "skipped":
      return t("enable.global.outcomeSkipped");
    case "failed":
      return t("enable.global.outcomeFailed");
    default:
      return t("enable.global.outcomeNotAttempted");
  }
}
